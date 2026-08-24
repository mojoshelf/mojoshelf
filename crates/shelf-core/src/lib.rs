//! Types shared between the mojoshelf registry Worker and the `shelf` CLI.

use serde::{Deserialize, Serialize};

/// A book as listed by `GET /api/books`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookSummary {
    pub name: String,
    pub url: String,
    pub description: Option<String>,
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub latest_version: Option<String>,
}

/// One published version of a book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub commit_sha: String,
    pub published_at: String,
    pub dependencies: Vec<String>,
}

/// A book with its full version history, from `GET /api/books/:name`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookDetail {
    pub name: String,
    pub url: String,
    pub description: Option<String>,
    pub author: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Newest first.
    pub versions: Vec<VersionInfo>,
    /// Names of other books with a published version depending on this one.
    #[serde(default)]
    pub dependents: Vec<String>,
}

/// One entry of the flat install set from `GET /api/books/:name/resolve`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedBook {
    pub name: String,
    pub url: String,
    pub version: String,
    pub commit_sha: String,
}

/// Body of `POST /api/publish`. The first publish of a new name registers
/// the book, owned by the publishing author; `url` comes from the book
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

/// A book's `shelf.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub books: Vec<String>,
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
}
