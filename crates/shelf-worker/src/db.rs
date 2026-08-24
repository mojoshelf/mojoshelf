//! D1 queries. All ids are bound as f64 because D1 bindings go through
//! JavaScript numbers.

use serde::Deserialize;
use shelf_core::{BookDetail, BookSummary, VersionInfo};
use std::collections::HashMap;
use worker::wasm_bindgen::JsValue;
use worker::{D1Database, Result};

#[derive(Deserialize)]
pub struct BookRow {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub description: Option<String>,
    pub author_id: Option<i64>,
    pub author: Option<String>,
    /// Comma-separated in storage; split via `shelf_core::split_tags`.
    pub tags: Option<String>,
}

impl BookRow {
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

const BOOK_SELECT: &str = "SELECT b.id, b.name, b.url, b.description, b.author_id, b.tags, \
    a.github_login AS author FROM books b LEFT JOIN authors a ON a.id = b.author_id";

#[derive(Deserialize)]
pub struct VersionRow {
    pub id: i64,
    pub version: String,
    pub commit_sha: String,
    pub published_at: String,
}

pub async fn book_by_name(d1: &D1Database, name: &str) -> Result<Option<BookRow>> {
    d1.prepare(&format!("{BOOK_SELECT} WHERE b.name = ?1"))
        .bind(&[name.into()])?
        .first::<BookRow>(None)
        .await
}

pub async fn versions_of(d1: &D1Database, book_id: i64) -> Result<Vec<VersionRow>> {
    d1.prepare(
        "SELECT id, version, commit_sha, published_at FROM versions WHERE book_id = ?1",
    )
    .bind(&[JsValue::from(book_id as f64)])?
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
             JOIN books b ON b.id = d.depends_on_book_id WHERE d.version_id = ?1",
        )
        .bind(&[JsValue::from(version_id as f64)])?
        .all()
        .await?
        .results::<Row>()?;
    Ok(rows.into_iter().map(|r| r.name).collect())
}

pub async fn list_books(d1: &D1Database, q: &str) -> Result<Vec<BookSummary>> {
    let pattern = format!("%{q}%");
    let books = d1
        .prepare(&format!(
            "{BOOK_SELECT} WHERE ?1 = '' OR b.name LIKE ?2 OR b.description LIKE ?2 \
             OR b.tags LIKE ?2 ORDER BY b.name"
        ))
        .bind(&[q.into(), pattern.into()])?
        .all()
        .await?
        .results::<BookRow>()?;

    #[derive(Deserialize)]
    struct VRow {
        book_id: i64,
        version: String,
    }
    let versions = d1
        .prepare("SELECT book_id, version FROM versions")
        .all()
        .await?
        .results::<VRow>()?;
    let mut by_book: HashMap<i64, Vec<String>> = HashMap::new();
    for v in versions {
        by_book.entry(v.book_id).or_default().push(v.version);
    }

    Ok(books
        .into_iter()
        .map(|b| {
            let latest = by_book.get(&b.id).and_then(|vs| {
                shelf_core::latest_version(vs.iter().map(String::as_str)).map(str::to_string)
            });
            BookSummary {
                tags: b.tag_list(),
                name: b.name,
                url: b.url,
                description: b.description,
                author: b.author,
                latest_version: latest,
            }
        })
        .collect())
}

pub async fn book_detail(d1: &D1Database, name: &str) -> Result<Option<BookDetail>> {
    let Some(book) = book_by_name(d1, name).await? else {
        return Ok(None);
    };
    let mut rows = versions_of(d1, book.id).await?;
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
    Ok(Some(BookDetail {
        tags: book.tag_list(),
        dependents: dependents_of(d1, book.id).await?,
        name: book.name,
        url: book.url,
        description: book.description,
        author: book.author,
        versions,
    }))
}

/// Inserts a version and its dependency rows in one atomic batch.
pub async fn insert_version(
    d1: &D1Database,
    book_id: i64,
    version: &str,
    commit_sha: &str,
    dep_ids: &[i64],
) -> Result<()> {
    let book_id_js = JsValue::from(book_id as f64);
    let mut stmts = vec![d1
        .prepare("INSERT INTO versions (book_id, version, commit_sha) VALUES (?1, ?2, ?3)")
        .bind(&[book_id_js.clone(), version.into(), commit_sha.into()])?];
    for dep_id in dep_ids {
        stmts.push(
            d1.prepare(
                "INSERT INTO dependencies (version_id, depends_on_book_id) VALUES \
                 ((SELECT id FROM versions WHERE book_id = ?1 AND version = ?2), ?3)",
            )
            .bind(&[
                book_id_js.clone(),
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

pub async fn books_of_author(d1: &D1Database, author_id: i64) -> Result<Vec<BookRow>> {
    d1.prepare(&format!("{BOOK_SELECT} WHERE b.author_id = ?1 ORDER BY b.name"))
        .bind(&[JsValue::from(author_id as f64)])?
        .all()
        .await?
        .results::<BookRow>()
}

pub async fn create_book(
    d1: &D1Database,
    name: &str,
    url: &str,
    author_id: i64,
    description: Option<&str>,
    tags: &str,
) -> Result<()> {
    d1.prepare(
        "INSERT INTO books (name, url, author_id, description, tags) \
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
/// publishes.
pub async fn claim_book(
    d1: &D1Database,
    book_id: i64,
    url: &str,
    author_id: i64,
    description: Option<&str>,
    tags: &str,
) -> Result<()> {
    d1.prepare(
        "UPDATE books SET url = ?2, author_id = ?3, description = ?4, tags = ?5, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
    )
    .bind(&[
        JsValue::from(book_id as f64),
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

/// Names of other books with a published version depending on this book.
pub async fn dependents_of(d1: &D1Database, book_id: i64) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Row {
        name: String,
    }
    let rows = d1
        .prepare(
            "SELECT DISTINCT b.name AS name FROM dependencies d \
             JOIN versions v ON v.id = d.version_id \
             JOIN books b ON b.id = v.book_id \
             WHERE d.depends_on_book_id = ?1 AND v.book_id != ?1 ORDER BY b.name",
        )
        .bind(&[JsValue::from(book_id as f64)])?
        .all()
        .await?
        .results::<Row>()?;
    Ok(rows.into_iter().map(|r| r.name).collect())
}

/// How many versions of OTHER books declare a dependency on this book.
pub async fn dependent_count(d1: &D1Database, book_id: i64) -> Result<i64> {
    #[derive(Deserialize)]
    struct Row {
        n: i64,
    }
    let row = d1
        .prepare(
            "SELECT COUNT(*) AS n FROM dependencies d \
             JOIN versions v ON v.id = d.version_id \
             WHERE d.depends_on_book_id = ?1 AND v.book_id != ?1",
        )
        .bind(&[JsValue::from(book_id as f64)])?
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

pub async fn delete_book(d1: &D1Database, book_id: i64) -> Result<()> {
    let id = JsValue::from(book_id as f64);
    d1.batch(vec![
        d1.prepare(
            "DELETE FROM dependencies WHERE version_id IN \
             (SELECT id FROM versions WHERE book_id = ?1)",
        )
        .bind(&[id.clone()])?,
        d1.prepare("DELETE FROM versions WHERE book_id = ?1")
            .bind(&[id.clone()])?,
        d1.prepare("DELETE FROM books WHERE id = ?1").bind(&[id])?,
    ])
    .await?;
    Ok(())
}

pub async fn upsert_book(d1: &D1Database, name: &str, url: &str, description: &str) -> Result<()> {
    d1.prepare(
        "INSERT INTO books (name, url, description) VALUES (?1, ?2, ?3) \
         ON CONFLICT (name) DO UPDATE SET url = ?2, description = ?3, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
    )
    .bind(&[name.into(), url.into(), description.into()])?
    .run()
    .await?;
    Ok(())
}
