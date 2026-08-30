//! Mirror of the modular-community conda channel: package names + latest
//! versions from repodata.json, upserted as kind='channel' tins. Runs on a
//! cron trigger, and on demand via POST /api/sync-channel (publish-token
//! gated — the sync is idempotent).

use crate::db;
use serde::Deserialize;
use std::collections::HashMap;
use crate::located::Located;
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

/// The channel keeps artifacts of packages that were later renamed (e.g.
/// small-time → small_time), so both spellings show up in repodata. Names
/// that differ only by `-` vs `_` are the same library: keep the variant
/// whose latest version is newest (the rename is where releases continue),
/// preferring `_` on a tie, and drop the other so the prune pass below
/// removes its tin.
fn dedupe_renamed(latest: &mut HashMap<String, String>) {
    let mut keep: HashMap<String, String> = HashMap::new();
    for (name, version) in latest.iter() {
        let norm = name.replace('-', "_");
        let wins = match keep.get(&norm) {
            None => true,
            Some(other) => {
                let other_version = &latest[other];
                newer(version, other_version)
                    || (version == other_version && name.contains('_'))
            }
        };
        if wins {
            keep.insert(norm, name.clone());
        }
    }
    latest.retain(|name, _| keep.get(&name.replace('-', "_")) == Some(name));
}

#[cfg(test)]
mod dedupe_tests {
    use super::*;

    fn map(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
    }

    #[test]
    fn renamed_package_keeps_newer_variant() {
        let mut latest = map(&[("small-time", "0.0.1"), ("small_time", "26.2.0"), ("other", "1.0.0")]);
        dedupe_renamed(&mut latest);
        assert!(latest.contains_key("small_time"));
        assert!(!latest.contains_key("small-time"));
        assert!(latest.contains_key("other"));
    }

    #[test]
    fn rename_in_other_direction_also_wins() {
        let mut latest = map(&[("ember_lib", "2.0.0"), ("ember-lib", "3.1.0")]);
        dedupe_renamed(&mut latest);
        assert!(latest.contains_key("ember-lib"));
        assert!(!latest.contains_key("ember_lib"));
    }

    #[test]
    fn tie_prefers_underscore() {
        let mut latest = map(&[("a-b", "1.0.0"), ("a_b", "1.0.0")]);
        dedupe_renamed(&mut latest);
        assert!(latest.contains_key("a_b"));
        assert!(!latest.contains_key("a-b"));
    }

    #[test]
    fn distinct_names_untouched() {
        let mut latest = map(&[("csv-mojo", "1.0.0"), ("zlib_mojo", "2.0.0")]);
        dedupe_renamed(&mut latest);
        assert_eq!(latest.len(), 2);
    }
}

pub async fn sync(env: &Env) -> Result<String> {
    let mut latest: HashMap<String, String> = HashMap::new();
    for subdir in SUBDIRS {
        let url = format!("{CHANNEL_BASE}/{subdir}/repodata.json");
        let mut res = Fetch::Url(url.parse().map_err(|_| Error::RustError("bad url".into()))?)
            .send()
            .await.at()?;
        if res.status_code() != 200 {
            // Missing subdir is fine; anything else we note and continue.
            continue;
        }
        let data: RepoData = res.json().await.at()?;
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
    dedupe_renamed(&mut latest);

    let d1 = env.d1("DB")?;
    let mut mirrored = 0usize;
    for (name, version) in &latest {
        // Source tins own their names — but record that the channel now
        // serves the same name (a graduated tin), so divergence is visible.
        if let Some(existing) = db::tin_by_name(&d1, name).await.at()? {
            if existing.kind != "channel" {
                if existing.channel_version.as_deref() != Some(version.as_str()) {
                    db::set_source_channel_version(&d1, name, Some(version)).await.at()?;
                }
                continue;
            }
        }
        let url = format!("https://prefix.dev/channels/modular-community/packages/{name}");
        db::upsert_channel_tin(&d1, name, &url, version).await.at()?;
        mirrored += 1;
    }

    // Prune channel tins that left the channel.
    let mut pruned = 0usize;
    for name in db::channel_tin_names(&d1).await.at()? {
        if !latest.contains_key(&name) {
            db::delete_channel_tin(&d1, &name).await.at()?;
            pruned += 1;
        }
    }

    // Graduated tins whose channel package disappeared: clear the marker.
    for name in db::graduated_source_tin_names(&d1).await.at()? {
        if !latest.contains_key(&name) {
            db::set_source_channel_version(&d1, &name, None).await.at()?;
        }
    }

    let enriched = match enrich(env, &d1).await {
        Ok(n) => n.to_string(),
        Err(e) => phase_failed("enrich", e).await,
    };
    let liveliness = match refresh_liveliness(env, &d1).await {
        Ok(n) => n,
        Err(e) => phase_failed("liveliness", e).await,
    };
    let cards = match refresh_cards(env, &d1).await {
        Ok(n) => n.to_string(),
        Err(e) => phase_failed("cards", e).await,
    };

    Ok(format!(
        "mirrored {mirrored} channel packages, pruned {pruned}, enriched {enriched}, \
         liveliness {liveliness}, cards {cards}"
    ))
}

/// A sync phase failed. The other phases still run — one broken phase should
/// not stop the rest — but the failure is reported rather than folded into the
/// summary string, where `liveliness ERROR: …` sat inside an otherwise
/// successful sync and went unnoticed for days.
async fn phase_failed(phase: &'static str, e: Error) -> String {
    let raw = e.to_string();
    let (message, location) = crate::located::split(&raw);
    crate::posthog_exception(
        "SyncPhaseError",
        message.to_string(),
        location.map(str::to_string),
        serde_json::json!({ "phase": phase }),
    )
    .await;
    console_log!("channel sync phase {phase} FAILED: {message}");
    format!("ERROR: {message}")
}

/// Repos refreshed per sync: 2 GitHub calls each, sized to stay inside the
/// Workers subrequest cap next to the mirror + enrichment fetches.
const LIVELINESS_BATCH: usize = 10;

async fn github_json<T: for<'de> serde::Deserialize<'de>>(
    env: &Env,
    url: &str,
) -> Result<Option<T>> {
    let mut headers = Headers::new();
    headers.set("User-Agent", "mojoshelf-sync")?;
    headers.set("Accept", "application/vnd.github+json")?;
    if let Ok(token) = env.secret("GITHUB_TOKEN") {
        headers.set("Authorization", &format!("Bearer {}", token.to_string()))?;
    }
    let mut init = RequestInit::new();
    init.with_headers(headers);
    let req = Request::new_with_init(url, &init)?;
    let mut res = Fetch::Request(req).send().await.at()?;
    match res.status_code() {
        200 => Ok(Some(res.json().await.at()?)),
        // 202: stats still computing. 404: repo gone or private. Both are
        // ordinary outcomes for a particular repo, so the caller skips it.
        202 | 404 => Ok(None),
        // Everything else is a problem with the request itself rather than
        // with this repo — above all 401, which is what an expired
        // GITHUB_TOKEN returns for every call, public repos included.
        // Folding these into None made a broken token look like a registry
        // where nothing ever needed refreshing.
        // GitHub explains itself in the body — "API rate limit exceeded" reads
        // very differently from "Resource not accessible by personal access
        // token" — and the two need opposite fixes.
        status => {
            let detail = res
                .text()
                .await
                .map(|b| b.chars().take(180).collect::<String>())
                .unwrap_or_default();
            Err(Error::RustError(format!(
                "github returned {status} for {url}: {detail}"
            )))
        }
    }
}

fn owner_repo(url: &str) -> Option<String> {
    let rest = url.split("github.com/").nth(1)?;
    let mut parts = rest.trim_end_matches(".git").split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Stars, last push, and commit counts (last month / last year) for the
/// stalest GitHub-hosted tins. Rate-limit friendly: 403s just leave the
/// batch for the next cycle; set GITHUB_TOKEN (worker secret) for 5000/hr.
async fn refresh_liveliness(env: &Env, d1: &worker::D1Database) -> Result<String> {
    #[derive(Deserialize)]
    struct Repo {
        stargazers_count: i64,
        forks_count: i64,
        pushed_at: String,
    }
    #[derive(Deserialize)]
    struct Participation {
        all: Vec<i64>,
    }
    let mut refreshed = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for (name, url) in db::stale_liveliness_tins(d1, LIVELINESS_BATCH).await.at()? {
        let Some(or) = owner_repo(&url) else { continue };
        let url = format!("https://api.github.com/repos/{or}");
        // Whatever happens to one repo, stamp it and move on. The stale queue
        // puts unrefreshed tins first, so a repo this token cannot read would
        // otherwise sit at the head of every batch and starve the rest — which
        // is exactly what froze the refresh once a batch of unreadable repos
        // was published.
        let repo = match github_json::<Repo>(env, &url).await {
            Ok(Some(repo)) => repo,
            Ok(None) => {
                db::touch_liveliness(d1, &name).await.at()?;
                continue;
            }
            Err(e) => {
                db::touch_liveliness(d1, &name).await.at()?;
                console_log!("liveliness {name}: {e}");
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        // 52 weekly commit counts, newest last; one call covers both windows.
        let (month, year) = match github_json::<Participation>(
            env,
            &format!("https://api.github.com/repos/{or}/stats/participation"),
        )
        .await.at()?
        {
            Some(p) => {
                let weeks = &p.all;
                let month: i64 = weeks.iter().rev().take(4).sum();
                let year: i64 = weeks.iter().sum();
                (Some(month), Some(year))
            }
            None => (None, None),
        };
        let score = interestingness(
            repo.stargazers_count,
            repo.forks_count,
            month,
            year,
            &repo.pushed_at,
        );
        db::set_liveliness(
            d1,
            &name,
            repo.stargazers_count,
            repo.forks_count,
            score,
            &repo.pushed_at,
            month,
            year,
        )
        .await
        .at()?;
        refreshed += 1;
    }
    // Nothing readable at all is a configuration problem, not a quiet day:
    // surface it. A partial failure is reported in the summary instead, so one
    // unreadable repo does not fail the whole sync.
    if refreshed == 0 && !failures.is_empty() {
        return Err(Error::RustError(format!(
            "all {} repos failed, first: {}",
            failures.len(),
            failures[0]
        )));
    }
    Ok(if failures.is_empty() {
        refreshed.to_string()
    } else {
        format!("{refreshed} ({} failed, first: {})", failures.len(), failures[0])
    })
}

/// How interesting a tin is, from its GitHub signals. Ranks the public list.
///
/// Each count goes through `ln(1 + n)` first, because the raw numbers live on
/// very different scales — a registry where the busiest repo has 19 stars but
/// hundreds of commits a year would otherwise be ranked by commits alone, and
/// a single runaway repo would bury everything else. Compressed this way, the
/// weights below mean what they say: a star is worth more than a fork, which
/// is worth more than a commit.
///
/// Recency is a bonus rather than a factor, so a long-dormant but widely used
/// library still places well, while an active one is pushed up.
fn interestingness(
    stars: i64,
    forks: i64,
    commits_month: Option<i64>,
    commits_year: Option<i64>,
    pushed_at: &str,
) -> f64 {
    let ln1p = |n: i64| (1.0 + n.max(0) as f64).ln();
    let recency = {
        let parsed = worker::js_sys::Date::parse(pushed_at);
        if parsed.is_nan() {
            0.0
        } else {
            let days = ((worker::js_sys::Date::now() - parsed) / 86_400_000.0).max(0.0);
            match days {
                d if d <= 30.0 => 3.0,
                d if d <= 90.0 => 1.5,
                d if d <= 365.0 => 0.5,
                _ => 0.0,
            }
        }
    };
    2.0 * ln1p(stars)
        + 1.5 * ln1p(forks)
        + 1.2 * ln1p(commits_month.unwrap_or(0))
        + 0.6 * ln1p(commits_year.unwrap_or(0))
        + recency
}

/// Agent cards rebuilt per sync (oldest first, like liveliness). Each
/// source tin costs up to ~9 fetches (tree + README + pixi.toml + a few
/// source files); channel tins cost none.
const CARD_BATCH: usize = 4;
const CARD_SRC_FILES: usize = 6;

/// Rebuilds the precomputed markdown card for the stalest tins.
async fn refresh_cards(env: &Env, d1: &worker::D1Database) -> Result<usize> {
    let mut built = 0usize;
    for name in db::stale_card_tins(d1, CARD_BATCH).await.at()? {
        let Some(detail) = db::tin_detail(d1, &name).await.at()? else {
            continue;
        };
        let extras = if detail.kind == "channel" {
            // Everything a channel card says is already in D1.
            shelf_core::cards::CardExtras::default()
        } else {
            source_extras(env, &detail).await
        };
        let card = shelf_core::cards::assemble_card(&detail, &extras);
        db::set_card(d1, &name, &card).await.at()?;
        built += 1;
    }
    Ok(built)
}

/// Repo-derived card extras for a source tin, read at the latest published
/// commit (HEAD when nothing is published). Every failed fetch degrades to
/// a metadata-only card instead of failing the sync.
async fn source_extras(env: &Env, detail: &shelf_core::TinDetail) -> shelf_core::cards::CardExtras {
    let mut extras = shelf_core::cards::CardExtras::default();
    let Some(or) = owner_repo(&detail.url) else {
        return extras;
    };
    let rev = detail
        .versions
        .first()
        .map(|v| v.commit_sha.clone())
        .unwrap_or_else(|| "HEAD".into());

    #[derive(Deserialize)]
    struct Tree {
        tree: Vec<TreeEntry>,
    }
    #[derive(Deserialize)]
    struct TreeEntry {
        path: String,
        #[serde(rename = "type")]
        kind: String,
    }
    let tree_url = format!("https://api.github.com/repos/{or}/git/trees/{rev}?recursive=1");
    let Ok(Some(tree)) = github_json::<Tree>(env, &tree_url).await else {
        return extras;
    };
    let files: Vec<&str> = tree
        .tree
        .iter()
        .filter(|e| e.kind == "blob")
        .map(|e| e.path.as_str())
        .collect();
    let raw = |path: &str| format!("https://raw.githubusercontent.com/{or}/{rev}/{path}");

    if let Some(readme) = files
        .iter()
        .find(|p| p.eq_ignore_ascii_case("readme.md") || p.eq_ignore_ascii_case("readme"))
    {
        if let Ok(Some(text)) = fetch_text(&raw(readme)).await {
            extras.snippet = shelf_core::cards::extract_snippet(&text);
        }
    }
    if files.iter().any(|p| *p == "pixi.toml") {
        if let Ok(Some(text)) = fetch_text(&raw("pixi.toml")).await {
            extras.import_name = shelf_core::cards::pixi_import_name(&text);
        }
    }
    let mut mojo: Vec<&str> = files
        .iter()
        .filter(|p| p.starts_with("src/") && p.ends_with(".mojo") && !p.contains("test"))
        .copied()
        .collect();
    // Shallow paths first: package roots and __init__ re-exports beat deep
    // internals when only a few files fit the budget.
    mojo.sort_by_key(|p| (p.matches('/').count(), p.to_string()));
    for path in mojo.into_iter().take(CARD_SRC_FILES) {
        if let Ok(Some(text)) = fetch_text(&raw(path)).await {
            let sigs = shelf_core::cards::extract_signatures(&text);
            if !sigs.is_empty() {
                extras.api.push((path.to_string(), sigs));
            }
        }
    }
    extras
}

const RECIPES_REPO: &str = "modular/modular-community";
/// Recipe fetches per sync — stays well inside the Workers subrequest cap;
/// the mirror converges over a couple of runs and is steady-state free.
const ENRICH_BATCH: usize = 20;

async fn fetch_text(url: &str) -> Result<Option<String>> {
    let mut res = Fetch::Url(url.parse().map_err(|_| Error::RustError("bad url".into()))?)
        .send()
        .await.at()?;
    if res.status_code() != 200 {
        return Ok(None);
    }
    Ok(Some(res.text().await.at()?))
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
async fn enrich(env: &Env, d1: &worker::D1Database) -> Result<usize> {
    let pending = db::unenriched_channel_tins(d1, ENRICH_BATCH).await.at()?;
    if pending.is_empty() {
        return Ok(0);
    }

    #[derive(Deserialize)]
    struct DirEntry {
        name: String,
    }
    let listing_url =
        format!("https://api.github.com/repos/{RECIPES_REPO}/contents/recipes");
    let dirs: Vec<DirEntry> = github_json(env, &listing_url)
        .await.at()?
        .ok_or_else(|| Error::RustError("recipes listing unavailable".into()))?;
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
            db::enrich_channel_tin(d1, &name, "", None, None).await.at()?;
            continue;
        };
        let raw_url = format!(
            "https://raw.githubusercontent.com/{RECIPES_REPO}/main/recipes/{dir}/recipe.yaml"
        );
        let Some(recipe) = fetch_text(&raw_url).await.at()? else {
            db::enrich_channel_tin(d1, &name, "", None, None).await.at()?;
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
        .await.at()?;
        enriched += 1;
    }
    Ok(enriched)
}
