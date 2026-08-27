//! Types shared between the mojoshelf registry Worker and the `shelf` CLI.

pub mod cards;

use serde::{Deserialize, Serialize};

/// A tin as listed by `GET /api/tins`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TinSummary {
    pub name: String,
    pub url: String,
    pub description: Option<String>,
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub latest_version: Option<String>,
    /// "source" (git-pinned tin) or "channel" (mirrored modular-community
    /// binary package).
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub stars: Option<i64>,
    /// ISO timestamp of the repo's last push (GitHub `pushed_at`).
    #[serde(default)]
    pub last_push: Option<String>,
    /// The URL the tin pointed at before its most recent URL change.
    #[serde(default)]
    pub prev_url: Option<String>,
    /// ISO timestamp of the most recent URL change (repo-swap warning).
    #[serde(default)]
    pub url_changed_at: Option<String>,
    /// ISO timestamp of the last tin-smoke consumer build check.
    #[serde(default)]
    pub verified_at: Option<String>,
    /// Whether the last tin-smoke check passed on every platform.
    #[serde(default)]
    pub verified_ok: Option<bool>,
    /// mojo-compiler version the last check built against (best effort).
    #[serde(default)]
    pub verified_compiler: Option<String>,
}

pub fn default_kind() -> String {
    "source".into()
}

/// One published version of a tin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub commit_sha: String,
    pub published_at: String,
    pub dependencies: Vec<String>,
}

/// A tin with its full version history, from `GET /api/tins/:name`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TinDetail {
    pub name: String,
    pub url: String,
    pub description: Option<String>,
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Newest first.
    pub versions: Vec<VersionInfo>,
    /// Names of other tins with a published version depending on this one.
    #[serde(default)]
    pub dependents: Vec<String>,
    #[serde(default = "default_kind")]
    pub kind: String,
    /// For kind "channel": the latest version on the channel.
    #[serde(default)]
    pub channel_version: Option<String>,
    #[serde(default)]
    pub stars: Option<i64>,
    #[serde(default)]
    pub last_push: Option<String>,
    #[serde(default)]
    pub commits_month: Option<i64>,
    #[serde(default)]
    pub commits_year: Option<i64>,
    /// The URL the tin pointed at before its most recent URL change.
    #[serde(default)]
    pub prev_url: Option<String>,
    /// ISO timestamp of the most recent URL change (repo-swap warning).
    #[serde(default)]
    pub url_changed_at: Option<String>,
    /// ISO timestamp of the last tin-smoke consumer build check.
    #[serde(default)]
    pub verified_at: Option<String>,
    /// Whether the last tin-smoke check passed on every platform.
    #[serde(default)]
    pub verified_ok: Option<bool>,
    /// mojo-compiler version the last check built against (best effort).
    #[serde(default)]
    pub verified_compiler: Option<String>,
}

/// One entry of the flat install set from `GET /api/tins/:name/resolve`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedTin {
    pub name: String,
    pub url: String,
    pub version: String,
    /// Empty for kind "channel" (no git pin — the conda solver owns it).
    pub commit_sha: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    /// The URL the tin pointed at before its most recent URL change.
    #[serde(default)]
    pub prev_url: Option<String>,
    /// ISO timestamp of the most recent URL change (repo-swap warning).
    #[serde(default)]
    pub url_changed_at: Option<String>,
}

/// Body of `POST /api/publish`. The first publish of a new name registers
/// the tin, owned by the publishing author; `url` comes from the tin
/// repo's origin remote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishRequest {
    pub name: String,
    pub version: String,
    pub commit_sha: String,
    pub url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub dependencies: Vec<String>,
}

/// Error body returned by the registry API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

/// A tin's `shelf.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Dependencies. `books` is accepted as a legacy alias — manifests
    /// published before the book→tin rename keep working.
    #[serde(default, alias = "books")]
    pub tins: Vec<String>,
}

/// Picks the highest semver from an iterator of version strings.
/// Non-semver strings are ignored.
pub fn latest_version<'a>(versions: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    versions
        .into_iter()
        .filter_map(|v| semver::Version::parse(v).ok().map(|p| (p, v)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, v)| v)
}

/// Splits a stored/submitted comma-separated tag string into clean tags.
pub fn split_tags(s: &str) -> Vec<String> {
    s.split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

pub fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// How long a repo-swap warning stays visible after a tin's URL changes.
pub const URL_CHANGE_WARN_DAYS: i64 = 30;

/// Unix seconds for a registry ISO-8601 UTC timestamp
/// ("YYYY-MM-DDTHH:MM:SSZ"). Hand-rolled (days-from-civil) so both the
/// wasm Worker and the native CLI share it without a date dependency.
pub fn iso_to_unix_secs(iso: &str) -> Option<i64> {
    let field = |range: std::ops::Range<usize>| iso.get(range)?.parse::<i64>().ok();
    let (y, m, d) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hh, mm, ss) = (field(11..13)?, field(14..16)?, field(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3_600 + mm * 60 + ss)
}

/// True while a URL change is recent enough to warrant a warning.
pub fn url_change_is_recent(changed_at: &str, now_unix_secs: i64) -> bool {
    iso_to_unix_secs(changed_at)
        .map(|t| now_unix_secs - t < URL_CHANGE_WARN_DAYS * 86_400)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_picks_semver_max() {
        let vs = ["0.9.0", "0.10.0", "0.2.1"];
        assert_eq!(latest_version(vs.iter().copied()), Some("0.10.0"));
    }

    #[test]
    fn latest_ignores_garbage() {
        let vs = ["not-a-version"];
        assert_eq!(latest_version(vs.iter().copied()), None);
    }

    #[test]
    fn sha_check() {
        assert!(is_full_sha(&"a".repeat(40)));
        assert!(!is_full_sha("abc123"));
        assert!(!is_full_sha(&"g".repeat(40)));
    }

    #[test]
    fn iso_parse_known_values() {
        assert_eq!(iso_to_unix_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_to_unix_secs("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(iso_to_unix_secs("2026-08-26T12:30:05Z"), Some(1_787_747_405));
        assert_eq!(iso_to_unix_secs("garbage"), None);
        assert_eq!(iso_to_unix_secs(""), None);
    }

    #[test]
    fn url_change_warning_expires_after_a_month() {
        let changed = "2026-08-01T00:00:00Z";
        let changed_secs = iso_to_unix_secs(changed).unwrap();
        assert!(url_change_is_recent(changed, changed_secs));
        assert!(url_change_is_recent(changed, changed_secs + 29 * 86_400));
        assert!(!url_change_is_recent(changed, changed_secs + 30 * 86_400));
        assert!(!url_change_is_recent("garbage", changed_secs));
    }
}
