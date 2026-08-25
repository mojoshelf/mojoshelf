//! Mirror of the modular-community conda channel: package names + latest
//! versions from repodata.json, upserted as kind='channel' tins. Runs on a
//! cron trigger, and on demand via POST /api/sync-channel (publish-token
//! gated — the sync is idempotent).

use crate::db;
use serde::Deserialize;
use std::collections::HashMap;
use worker::*;

const CHANNEL_BASE: &str = "https://repo.prefix.dev/modular-community";
const SUBDIRS: [&str; 4] = ["noarch", "osx-arm64", "linux-64", "linux-aarch64"];

#[derive(Deserialize)]
struct RepoData {
    #[serde(default)]
    packages: HashMap<String, PkgEntry>,
    #[serde(default, rename = "packages.conda")]
    packages_conda: HashMap<String, PkgEntry>,
}

#[derive(Deserialize)]
struct PkgEntry {
    name: String,
    version: String,
}

/// Lenient "is a newer than b": semver when both parse, else lexicographic.
fn newer(a: &str, b: &str) -> bool {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a > b,
        _ => a > b,
    }
}

pub async fn sync(env: &Env) -> Result<String> {
    let mut latest: HashMap<String, String> = HashMap::new();
    for subdir in SUBDIRS {
        let url = format!("{CHANNEL_BASE}/{subdir}/repodata.json");
        let mut res = Fetch::Url(url.parse().map_err(|_| Error::RustError("bad url".into()))?)
            .send()
            .await?;
        if res.status_code() != 200 {
            // Missing subdir is fine; anything else we note and continue.
            continue;
        }
        let data: RepoData = res.json().await?;
        for entry in data.packages.values().chain(data.packages_conda.values()) {
            match latest.get(&entry.name) {
                Some(v) if !newer(&entry.version, v) => {}
                _ => {
                    latest.insert(entry.name.clone(), entry.version.clone());
                }
            }
        }
    }
    if latest.is_empty() {
        return Err(Error::RustError("channel repodata yielded no packages".into()));
    }

    let d1 = env.d1("DB")?;
    let mut mirrored = 0usize;
    for (name, version) in &latest {
        // Source tins own their names — but record that the channel now
        // serves the same name (a graduated tin), so divergence is visible.
        if let Some(existing) = db::tin_by_name(&d1, name).await? {
            if existing.kind != "channel" {
                if existing.channel_version.as_deref() != Some(version.as_str()) {
                    db::set_source_channel_version(&d1, name, Some(version)).await?;
                }
                continue;
            }
        }
        let url = format!("https://prefix.dev/channels/modular-community/packages/{name}");
        db::upsert_channel_tin(&d1, name, &url, version).await?;
        mirrored += 1;
    }

    // Prune channel tins that left the channel.
    let mut pruned = 0usize;
    for name in db::channel_tin_names(&d1).await? {
        if !latest.contains_key(&name) {
            db::delete_channel_tin(&d1, &name).await?;
            pruned += 1;
        }
    }

    // Graduated tins whose channel package disappeared: clear the marker.
    for name in db::graduated_source_tin_names(&d1).await? {
        if !latest.contains_key(&name) {
            db::set_source_channel_version(&d1, &name, None).await?;
        }
    }

    let enriched = match enrich(&d1).await {
        Ok(n) => n.to_string(),
        Err(e) => format!("ERROR: {e}"),
    };

    Ok(format!(
        "mirrored {mirrored} channel packages, pruned {pruned}, enriched {enriched}"
    ))
}

const RECIPES_REPO: &str = "modular/modular-community";
/// Recipe fetches per sync — stays well inside the Workers subrequest cap;
/// the mirror converges over a couple of runs and is steady-state free.
const ENRICH_BATCH: usize = 20;

async fn fetch_text(url: &str) -> Result<Option<String>> {
    let mut res = Fetch::Url(url.parse().map_err(|_| Error::RustError("bad url".into()))?)
        .send()
        .await?;
    if res.status_code() != 200 {
        return Ok(None);
    }
    Ok(Some(res.text().await?))
}

fn yaml_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(key) {
            let v = rest.trim().trim_matches('"').trim_matches('\'').to_string();
            if !v.is_empty() && !v.contains("${{") {
                return Some(v);
            }
        }
    }
    None
}

/// First `- entry` after a `maintainers:` line.
fn first_maintainer(text: &str) -> Option<String> {
    let mut in_list = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("maintainers:") {
            in_list = true;
            continue;
        }
        if in_list {
            if let Some(entry) = trimmed.strip_prefix("- ") {
                return Some(entry.trim().trim_matches('"').to_string());
            }
            return None;
        }
    }
    None
}

fn repo_owner(url: &str) -> Option<String> {
    let rest = url.split("github.com/").nth(1)?;
    let owner = rest.split('/').next()?.trim();
    if owner.is_empty() {
        None
    } else {
        Some(owner.to_string())
    }
}

/// Fill maintainer/description/repository for channel tins that lack them,
/// from the modular-community recipe files. Dir names are matched
/// case-insensitively to package names.
async fn enrich(d1: &worker::D1Database) -> Result<usize> {
    let pending = db::unenriched_channel_tins(d1, ENRICH_BATCH).await?;
    if pending.is_empty() {
        return Ok(0);
    }

    #[derive(Deserialize)]
    struct DirEntry {
        name: String,
    }
    let listing_url =
        format!("https://api.github.com/repos/{RECIPES_REPO}/contents/recipes");
    let mut headers = Headers::new();
    headers.set("User-Agent", "mojoshelf-sync")?;
    let mut init = RequestInit::new();
    init.with_headers(headers);
    let req = Request::new_with_init(&listing_url, &init)?;
    let mut res = Fetch::Request(req).send().await?;
    if res.status_code() != 200 {
        return Err(Error::RustError(format!(
            "recipes listing returned {}",
            res.status_code()
        )));
    }
    let dirs: Vec<DirEntry> = res.json().await?;
    // Match package name to recipe dir with increasing looseness: exact
    // (case-insensitive), separator-insensitive, then with mojo affixes
    // stripped from the dir name (mojo-libc -> libc, mosaic-mojo -> mosaic).
    fn norm(s: &str) -> String {
        s.to_lowercase().replace(['-', '_'], "")
    }
    fn strip_affix(s: &str) -> String {
        let n = norm(s);
        n.strip_prefix("mojo")
            .or_else(|| n.strip_suffix("mojo"))
            .map(str::to_string)
            .unwrap_or(n)
    }
    let find_dir = |name: &str| -> Option<String> {
        let lower = name.to_lowercase();
        let normed = norm(name);
        dirs.iter()
            .find(|d| d.name.to_lowercase() == lower)
            .or_else(|| dirs.iter().find(|d| norm(&d.name) == normed))
            .or_else(|| dirs.iter().find(|d| strip_affix(&d.name) == normed))
            .map(|d| d.name.clone())
    };

    let mut enriched = 0usize;
    for name in pending {
        let Some(dir) = find_dir(&name) else {
            // No recipe dir matches: mark checked so we don't retry forever.
            db::enrich_channel_tin(d1, &name, "", None, None).await?;
            continue;
        };
        let raw_url = format!(
            "https://raw.githubusercontent.com/{RECIPES_REPO}/main/recipes/{dir}/recipe.yaml"
        );
        let Some(recipe) = fetch_text(&raw_url).await? else {
            db::enrich_channel_tin(d1, &name, "", None, None).await?;
            continue;
        };
        let repository = yaml_value(&recipe, "repository:")
            .map(|r| r.trim_end_matches(".git").to_string())
            .or_else(|| yaml_value(&recipe, "homepage:"));
        let author = first_maintainer(&recipe)
            .or_else(|| repository.as_deref().and_then(repo_owner))
            .unwrap_or_default();
        let summary = yaml_value(&recipe, "summary:");
        db::enrich_channel_tin(
            d1,
            &name,
            &author,
            summary.as_deref(),
            repository.as_deref(),
        )
        .await?;
        enriched += 1;
    }
    Ok(enriched)
}
