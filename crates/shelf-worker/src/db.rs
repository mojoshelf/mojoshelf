//! D1 queries. All ids are bound as f64 because D1 bindings go through
//! JavaScript numbers.

use serde::Deserialize;
use shelf_core::{TinDetail, TinSummary, VersionInfo};
use std::collections::HashMap;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, Result};

#[derive(Deserialize)]
pub struct TinRow {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub description: Option<String>,
    pub author_id: Option<i64>,
    pub author: Option<String>,
    /// Comma-separated in storage; split via `shelf_core::split_tags`.
    pub tags: Option<String>,
    pub kind: String,
    pub channel_version: Option<String>,
    pub channel_author: Option<String>,
    pub stars: Option<i64>,
    pub last_push: Option<String>,
    pub commits_month: Option<i64>,
    pub commits_year: Option<i64>,
    pub prev_url: Option<String>,
    pub url_changed_at: Option<String>,
    pub verified_at: Option<String>,
    pub verified_ok: Option<i64>,
    pub verified_compiler: Option<String>,
    pub nightly_at: Option<String>,
    pub nightly_ok: Option<i64>,
    pub nightly_compiler: Option<String>,
}

impl TinRow {
    pub fn tag_list(&self) -> Vec<String> {
        shelf_core::split_tags(self.tags.as_deref().unwrap_or(""))
    }
}

#[derive(Deserialize)]
pub struct AuthorRow {
    pub id: i64,
    #[allow(dead_code)] // part of the row shape; selected but not read yet
    pub github_id: i64,
    pub github_login: String,
    pub token_hash: Option<String>,
}

const TIN_SELECT: &str = "SELECT b.id, b.name, b.url, b.description, b.author_id, b.tags, \
    b.kind, b.channel_version, b.channel_author, \
    b.stars, b.last_push, b.commits_month, b.commits_year, \
    b.prev_url, b.url_changed_at, \
    b.verified_at, b.verified_ok, b.verified_compiler, \
    b.nightly_at, b.nightly_ok, b.nightly_compiler, \
    a.github_login AS author FROM tins b LEFT JOIN authors a ON a.id = b.author_id";

#[derive(Deserialize)]
pub struct VersionRow {
    pub id: i64,
    pub version: String,
    pub commit_sha: String,
    pub published_at: String,
}

pub async fn tin_by_name(d1: &D1Database, name: &str) -> Result<Option<TinRow>> {
    d1.prepare(&format!("{TIN_SELECT} WHERE b.name = ?1"))
        .bind(&[name.into()])?
        .first::<TinRow>(None)
        .await
}

pub async fn versions_of(d1: &D1Database, tin_id: i64) -> Result<Vec<VersionRow>> {
    d1.prepare(
        "SELECT id, version, commit_sha, published_at FROM versions WHERE tin_id = ?1",
    )
    .bind(&[JsValue::from(tin_id as f64)])?
    .all()
    .await?
    .results::<VersionRow>()
}

pub async fn dependency_names(d1: &D1Database, version_id: i64) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
    }
    let rows = d1
        .prepare(
            "SELECT b.name AS name FROM dependencies d \
             JOIN tins b ON b.id = d.depends_on_tin_id WHERE d.version_id = ?1",
        )
        .bind(&[JsValue::from(version_id as f64)])?
        .all()
        .await?
        .results::<Row>()?;
    Ok(rows.into_iter().map(|r| r.name).collect())
}

pub async fn list_tins(d1: &D1Database, q: &str) -> Result<Vec<TinSummary>> {
    let pattern = format!("%{q}%");
    let tins = d1
        .prepare(&format!(
            "{TIN_SELECT} WHERE ?1 = '' OR b.name LIKE ?2 OR b.description LIKE ?2 \
             OR b.tags LIKE ?2 ORDER BY b.name"
        ))
        .bind(&[q.into(), pattern.into()])?
        .all()
        .await?
        .results::<TinRow>()?;

    #[derive(Deserialize)]
    struct VRow {
        tin_id: i64,
        version: String,
    }
    let versions = d1
        .prepare("SELECT tin_id, version FROM versions")
        .all()
        .await?
        .results::<VRow>()?;
    let mut by_tin: HashMap<i64, Vec<String>> = HashMap::new();
    for v in versions {
        by_tin.entry(v.tin_id).or_default().push(v.version);
    }

    Ok(tins
        .into_iter()
        .map(|b| {
            let latest = by_tin.get(&b.id).and_then(|vs| {
                shelf_core::latest_version(vs.iter().map(String::as_str)).map(str::to_string)
            });
            TinSummary {
                tags: b.tag_list(),
                latest_version: if b.kind == "channel" {
                    b.channel_version.clone()
                } else {
                    latest
                },
                author: if b.kind == "channel" {
                    b.channel_author.clone().filter(|a| !a.is_empty())
                } else {
                    b.author
                },
                kind: b.kind,
                stars: b.stars,
                last_push: b.last_push,
                prev_url: b.prev_url,
                url_changed_at: b.url_changed_at,
                verified_at: b.verified_at,
                verified_ok: b.verified_ok.map(|v| v != 0),
                verified_compiler: b.verified_compiler,
                nightly_at: b.nightly_at,
                nightly_ok: b.nightly_ok.map(|v| v != 0),
                nightly_compiler: b.nightly_compiler,
                name: b.name,
                url: b.url,
                description: b.description,
            }
        })
        .collect())
}

pub async fn tin_detail(d1: &D1Database, name: &str) -> Result<Option<TinDetail>> {
    let Some(tin) = tin_by_name(d1, name).await? else {
        return Ok(None);
    };
    let mut rows = versions_of(d1, tin.id).await?;
    rows.sort_by(|a, b| {
        let pa = semver::Version::parse(&a.version);
        let pb = semver::Version::parse(&b.version);
        match (pa, pb) {
            (Ok(pa), Ok(pb)) => pb.cmp(&pa),
            _ => b.version.cmp(&a.version),
        }
    });
    let mut versions = Vec::with_capacity(rows.len());
    for row in rows {
        versions.push(VersionInfo {
            dependencies: dependency_names(d1, row.id).await?,
            version: row.version,
            commit_sha: row.commit_sha,
            published_at: row.published_at,
        });
    }
    Ok(Some(TinDetail {
        tags: tin.tag_list(),
        dependents: dependents_of(d1, tin.id).await?,
        author: if tin.kind == "channel" {
            tin.channel_author.clone().filter(|a| !a.is_empty())
        } else {
            tin.author
        },
        kind: tin.kind,
        channel_version: tin.channel_version,
        stars: tin.stars,
        last_push: tin.last_push,
        commits_month: tin.commits_month,
        commits_year: tin.commits_year,
        prev_url: tin.prev_url,
        url_changed_at: tin.url_changed_at,
        verified_at: tin.verified_at,
        verified_ok: tin.verified_ok.map(|v| v != 0),
        verified_compiler: tin.verified_compiler,
        nightly_at: tin.nightly_at,
        nightly_ok: tin.nightly_ok.map(|v| v != 0),
        nightly_compiler: tin.nightly_compiler,
        name: tin.name,
        url: tin.url,
        description: tin.description,
        versions,
    }))
}

/// Inserts a version and its dependency rows in one atomic batch.
pub async fn insert_version(
    d1: &D1Database,
    tin_id: i64,
    version: &str,
    commit_sha: &str,
    dep_ids: &[i64],
) -> Result<()> {
    let tin_id_js = JsValue::from(tin_id as f64);
    let mut stmts = vec![d1
        .prepare("INSERT INTO versions (tin_id, version, commit_sha) VALUES (?1, ?2, ?3)")
        .bind(&[tin_id_js.clone(), version.into(), commit_sha.into()])?];
    for dep_id in dep_ids {
        stmts.push(
            d1.prepare(
                "INSERT INTO dependencies (version_id, depends_on_tin_id) VALUES \
                 ((SELECT id FROM versions WHERE tin_id = ?1 AND version = ?2), ?3)",
            )
            .bind(&[
                tin_id_js.clone(),
                version.into(),
                JsValue::from(*dep_id as f64),
            ])?,
        );
    }
    d1.batch(stmts).await?;
    Ok(())
}

pub async fn upsert_author(d1: &D1Database, github_id: i64, login: &str) -> Result<AuthorRow> {
    d1.prepare(
        "INSERT INTO authors (github_id, github_login) VALUES (?1, ?2) \
         ON CONFLICT (github_id) DO UPDATE SET github_login = ?2 \
         RETURNING id, github_id, github_login, token_hash",
    )
    .bind(&[JsValue::from(github_id as f64), login.into()])?
    .first::<AuthorRow>(None)
    .await?
    .ok_or_else(|| worker::Error::RustError("author upsert returned no row".into()))
}

pub async fn author_by_id(d1: &D1Database, id: i64) -> Result<Option<AuthorRow>> {
    d1.prepare("SELECT id, github_id, github_login, token_hash FROM authors WHERE id = ?1")
        .bind(&[JsValue::from(id as f64)])?
        .first::<AuthorRow>(None)
        .await
}

pub async fn author_by_token_hash(d1: &D1Database, hash: &str) -> Result<Option<AuthorRow>> {
    d1.prepare("SELECT id, github_id, github_login, token_hash FROM authors WHERE token_hash = ?1")
        .bind(&[hash.into()])?
        .first::<AuthorRow>(None)
        .await
}

pub async fn set_token_hash(d1: &D1Database, author_id: i64, hash: &str) -> Result<()> {
    d1.prepare("UPDATE authors SET token_hash = ?2 WHERE id = ?1")
        .bind(&[JsValue::from(author_id as f64), hash.into()])?
        .run()
        .await?;
    Ok(())
}

pub async fn tins_of_author(d1: &D1Database, author_id: i64) -> Result<Vec<TinRow>> {
    d1.prepare(&format!("{TIN_SELECT} WHERE b.author_id = ?1 ORDER BY b.name"))
        .bind(&[JsValue::from(author_id as f64)])?
        .all()
        .await?
        .results::<TinRow>()
}

pub async fn create_tin(
    d1: &D1Database,
    name: &str,
    url: &str,
    author_id: i64,
    description: Option<&str>,
    tags: &str,
) -> Result<()> {
    d1.prepare(
        "INSERT INTO tins (name, url, author_id, description, tags) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&[
        name.into(),
        url.into(),
        JsValue::from(author_id as f64),
        description.map(JsValue::from).unwrap_or(JsValue::NULL),
        tags.into(),
    ])?
    .run()
    .await?;
    Ok(())
}

/// Sets/refreshes ownership, URL, and shelf.toml metadata when an author
/// publishes. A URL that differs from the stored one is recorded in
/// prev_url/url_changed_at so consumers can be warned about the repo swap
/// (column references in SET read the pre-update row, so ordering is safe).
pub async fn claim_tin(
    d1: &D1Database,
    tin_id: i64,
    url: &str,
    author_id: i64,
    description: Option<&str>,
    tags: &str,
) -> Result<()> {
    d1.prepare(
        "UPDATE tins SET \
         prev_url = CASE WHEN url != ?2 THEN url ELSE prev_url END, \
         url_changed_at = CASE WHEN url != ?2 \
             THEN strftime('%Y-%m-%dT%H:%M:%SZ', 'now') ELSE url_changed_at END, \
         url = ?2, author_id = ?3, description = ?4, tags = ?5, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
    )
    .bind(&[
        JsValue::from(tin_id as f64),
        url.into(),
        JsValue::from(author_id as f64),
        description.map(JsValue::from).unwrap_or(JsValue::NULL),
        tags.into(),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn author_by_login(d1: &D1Database, login: &str) -> Result<Option<AuthorRow>> {
    d1.prepare("SELECT id, github_id, github_login, token_hash FROM authors WHERE github_login = ?1")
        .bind(&[login.into()])?
        .first::<AuthorRow>(None)
        .await
}

/// Names of other tins with a published version depending on this tin.
pub async fn dependents_of(d1: &D1Database, tin_id: i64) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
    }
    let rows = d1
        .prepare(
            "SELECT DISTINCT b.name AS name FROM dependencies d \
             JOIN versions v ON v.id = d.version_id \
             JOIN tins b ON b.id = v.tin_id \
             WHERE d.depends_on_tin_id = ?1 AND v.tin_id != ?1 ORDER BY b.name",
        )
        .bind(&[JsValue::from(tin_id as f64)])?
        .all()
        .await?
        .results::<Row>()?;
    Ok(rows.into_iter().map(|r| r.name).collect())
}

/// How many versions of OTHER tins declare a dependency on this tin.
pub async fn dependent_count(d1: &D1Database, tin_id: i64) -> Result<i64> {
    #[derive(Deserialize)]
    struct Row {
        n: i64,
    }
    let row = d1
        .prepare(
            "SELECT COUNT(*) AS n FROM dependencies d \
             JOIN versions v ON v.id = d.version_id \
             WHERE d.depends_on_tin_id = ?1 AND v.tin_id != ?1",
        )
        .bind(&[JsValue::from(tin_id as f64)])?
        .first::<Row>(None)
        .await?;
    Ok(row.map(|r| r.n).unwrap_or(0))
}

pub async fn delete_version(d1: &D1Database, version_id: i64) -> Result<()> {
    let id = JsValue::from(version_id as f64);
    d1.batch(vec![
        d1.prepare("DELETE FROM dependencies WHERE version_id = ?1")
            .bind(&[id.clone()])?,
        d1.prepare("DELETE FROM versions WHERE id = ?1").bind(&[id])?,
    ])
    .await?;
    Ok(())
}

pub async fn delete_tin(d1: &D1Database, tin_id: i64) -> Result<()> {
    let id = JsValue::from(tin_id as f64);
    d1.batch(vec![
        d1.prepare(
            "DELETE FROM dependencies WHERE version_id IN \
             (SELECT id FROM versions WHERE tin_id = ?1)",
        )
        .bind(&[id.clone()])?,
        d1.prepare("DELETE FROM versions WHERE tin_id = ?1")
            .bind(&[id.clone()])?,
        d1.prepare("DELETE FROM tins WHERE id = ?1").bind(&[id])?,
    ])
    .await?;
    Ok(())
}

/// Mirror one modular-community channel package as a kind='channel' tin.
/// The WHERE guard keeps source tins untouched on name conflicts.
pub async fn upsert_channel_tin(
    d1: &D1Database,
    name: &str,
    url: &str,
    version: &str,
) -> Result<()> {
    d1.prepare(
        "INSERT INTO tins (name, url, kind, channel_version) \
         VALUES (?1, ?2, 'channel', ?3) \
         ON CONFLICT (name) DO UPDATE SET channel_version = ?3, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE tins.kind = 'channel'",
    )
    .bind(&[name.into(), url.into(), version.into()])?
    .run()
    .await?;
    Ok(())
}

/// Recipe-derived metadata for a channel tin. Empty author = "checked,
/// none found" (prevents refetch loops).
pub async fn enrich_channel_tin(
    d1: &D1Database,
    name: &str,
    author: &str,
    description: Option<&str>,
    url: Option<&str>,
) -> Result<()> {
    d1.prepare(
        "UPDATE tins SET channel_author = ?2, \
         description = COALESCE(?3, description), \
         url = COALESCE(?4, url) \
         WHERE name = ?1 AND kind = 'channel'",
    )
    .bind(&[
        name.into(),
        author.into(),
        description.map(worker::wasm_bindgen::JsValue::from).unwrap_or(worker::wasm_bindgen::JsValue::NULL),
        url.map(worker::wasm_bindgen::JsValue::from).unwrap_or(worker::wasm_bindgen::JsValue::NULL),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn unenriched_channel_tins(d1: &D1Database, limit: usize) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
    }
    let rows = d1
        .prepare("SELECT name FROM tins WHERE kind = 'channel' AND channel_author IS NULL LIMIT ?1")
        .bind(&[worker::wasm_bindgen::JsValue::from(limit as f64)])?
        .all()
        .await?
        .results::<Row>()?;
    Ok(rows.into_iter().map(|r| r.name).collect())
}

/// A graduated source tin: record (or clear) the version the
/// modular-community channel serves under the same name.
pub async fn set_source_channel_version(
    d1: &D1Database,
    name: &str,
    version: Option<&str>,
) -> Result<()> {
    d1.prepare("UPDATE tins SET channel_version = ?2 WHERE name = ?1 AND kind = 'source'")
        .bind(&[
            name.into(),
            version.map(worker::wasm_bindgen::JsValue::from).unwrap_or(worker::wasm_bindgen::JsValue::NULL),
        ])?
        .run()
        .await?;
    Ok(())
}

pub async fn graduated_source_tin_names(d1: &D1Database) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
    }
    let rows = d1
        .prepare("SELECT name FROM tins WHERE kind = 'source' AND channel_version IS NOT NULL")
        .all()
        .await?
        .results::<Row>()?;
    Ok(rows.into_iter().map(|r| r.name).collect())
}

/// GitHub-hosted tins whose liveliness is oldest (or never fetched).
pub async fn stale_liveliness_tins(d1: &D1Database, limit: usize) -> Result<Vec<(String, String)>> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
        url: String,
    }
    let rows = d1
        .prepare(
            "SELECT name, url FROM tins WHERE url LIKE '%github.com%' \
             ORDER BY liveliness_at IS NOT NULL, liveliness_at ASC LIMIT ?1",
        )
        .bind(&[worker::wasm_bindgen::JsValue::from(limit as f64)])?
        .all()
        .await?
        .results::<Row>()?;
    Ok(rows.into_iter().map(|r| (r.name, r.url)).collect())
}

pub async fn set_liveliness(
    d1: &D1Database,
    name: &str,
    stars: i64,
    last_push: &str,
    commits_month: Option<i64>,
    commits_year: Option<i64>,
) -> Result<()> {
    d1.prepare(
        "UPDATE tins SET stars = ?2, last_push = ?3, \
         commits_month = COALESCE(?4, commits_month), \
         commits_year = COALESCE(?5, commits_year), \
         liveliness_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE name = ?1",
    )
    .bind(&[
        name.into(),
        worker::wasm_bindgen::JsValue::from(stars as f64),
        last_push.into(),
        commits_month.map(|v| worker::wasm_bindgen::JsValue::from(v as f64)).unwrap_or(worker::wasm_bindgen::JsValue::NULL),
        commits_year.map(|v| worker::wasm_bindgen::JsValue::from(v as f64)).unwrap_or(worker::wasm_bindgen::JsValue::NULL),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn channel_tin_names(d1: &D1Database) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
    }
    let rows = d1
        .prepare("SELECT name FROM tins WHERE kind = 'channel'")
        .all()
        .await?
        .results::<Row>()?;
    Ok(rows.into_iter().map(|r| r.name).collect())
}

pub async fn delete_channel_tin(d1: &D1Database, name: &str) -> Result<()> {
    d1.prepare("DELETE FROM tins WHERE name = ?1 AND kind = 'channel'")
        .bind(&[name.into()])?
        .run()
        .await?;
    Ok(())
}

pub async fn upsert_tin(d1: &D1Database, name: &str, url: &str, description: &str) -> Result<()> {
    d1.prepare(
        "INSERT INTO tins (name, url, description) VALUES (?1, ?2, ?3) \
         ON CONFLICT (name) DO UPDATE SET \
         prev_url = CASE WHEN tins.url != ?2 THEN tins.url ELSE tins.prev_url END, \
         url_changed_at = CASE WHEN tins.url != ?2 \
             THEN strftime('%Y-%m-%dT%H:%M:%SZ', 'now') ELSE tins.url_changed_at END, \
         url = ?2, description = ?3, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
    )
    .bind(&[name.into(), url.into(), description.into()])?
    .run()
    .await?;
    Ok(())
}

/// Tins whose agent card is oldest (or never built), refreshed in batches
/// by the sync cron like liveliness.
pub async fn stale_card_tins(d1: &D1Database, limit: usize) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
    }
    let rows = d1
        .prepare("SELECT name FROM tins ORDER BY card_at IS NOT NULL, card_at ASC LIMIT ?1")
        .bind(&[JsValue::from(limit as f64)])?
        .all()
        .await?
        .results::<Row>()?;
    Ok(rows.into_iter().map(|r| r.name).collect())
}

pub async fn set_card(d1: &D1Database, name: &str, card: &str) -> Result<()> {
    d1.prepare(
        "UPDATE tins SET card = ?2, card_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') \
         WHERE name = ?1",
    )
    .bind(&[name.into(), card.into()])?
    .run()
    .await?;
    Ok(())
}

/// The stored card for one tin; outer None = no such tin, inner None = no
/// card generated yet.
pub async fn card_of(d1: &D1Database, name: &str) -> Result<Option<Option<String>>> {
    #[derive(Deserialize)]
    struct Row {
        card: Option<String>,
    }
    Ok(d1
        .prepare("SELECT card FROM tins WHERE name = ?1")
        .bind(&[name.into()])?
        .first::<Row>(None)
        .await?
        .map(|r| r.card))
}

/// (name, description, url, card) for every tin — source tins first — for
/// /llms-full.txt.
pub async fn all_cards(d1: &D1Database) -> Result<Vec<(String, Option<String>, String, Option<String>)>> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
        description: Option<String>,
        url: String,
        card: Option<String>,
    }
    let rows = d1
        .prepare("SELECT name, description, url, card FROM tins ORDER BY kind = 'channel', name")
        .all()
        .await?
        .results::<Row>()?;
    Ok(rows
        .into_iter()
        .map(|r| (r.name, r.description, r.url, r.card))
        .collect())
}

/// Records one tin-smoke outcome reported by CI. `nightly` selects the
/// separate record for builds against the Mojo nightly channel.
pub async fn set_verified(
    d1: &D1Database,
    name: &str,
    ok: bool,
    compiler: Option<&str>,
    nightly: bool,
) -> Result<()> {
    let sql = if nightly {
        "UPDATE tins SET nightly_ok = ?2, nightly_compiler = ?3, \
         nightly_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE name = ?1"
    } else {
        "UPDATE tins SET verified_ok = ?2, verified_compiler = ?3, \
         verified_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE name = ?1"
    };
    d1.prepare(sql)
    .bind(&[
        name.into(),
        JsValue::from(if ok { 1.0 } else { 0.0 }),
        compiler.map(JsValue::from).unwrap_or(JsValue::NULL),
    ])?
    .run()
    .await?;
    Ok(())
}
