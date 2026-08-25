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
        // Source tins own their names: the upsert's WHERE guard already
        // protects them, but skip cleanly instead of relying on it.
        if let Some(existing) = db::tin_by_name(&d1, name).await? {
            if existing.kind != "channel" {
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

    Ok(format!(
        "mirrored {mirrored} channel packages, pruned {pruned}"
    ))
}
