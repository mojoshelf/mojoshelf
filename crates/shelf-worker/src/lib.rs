//! The mojoshelf registry Worker: public JSON API, public index page, and
//! admin pages. Admin and publish routes are gated by Cloudflare Access at
//! the edge; the in-Worker header check below is defense in depth only.

mod auth;
mod authors;
mod db;
mod html;

use serde_json::json;
use shelf_core::{is_full_sha, PublishRequest, ResolvedTin};
use std::collections::{HashSet, VecDeque};
use worker::*;

#[event(fetch)]
pub async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    Router::new()
        .get_async("/", home)
        .get_async("/getting-started", getting_started)
        .get_async("/install-modes", install_modes)
        .get_async("/packaging", packaging)
        .get_async("/build-process", packaging)
        .get_async("/tins/:name", tin_page)
        .get_async("/api/tins", api_list)
        .get_async("/api/tins/:name", api_tin)
        .get_async("/api/tins/:name/resolve", api_resolve)
        .post_async("/api/publish", api_publish)
        .get_async("/authors", authors::page)
        .get_async("/authors/:login", authors::author_page)
        .get_async("/auth/login", authors::login)
        .get_async("/auth/callback", authors::callback)
        .post_async("/auth/logout", authors::logout)
        .post_async("/authors/token", authors::generate_token)
        .post_async("/authors/tins/:name/delete", authors::delete_tin)
        .post_async(
            "/authors/tins/:name/versions/:version/delete",
            authors::delete_version,
        )
        .get_async("/admin", admin_page)
        .post_async("/admin/tins", admin_upsert)
        .run(req, env)
        .await
}

pub(crate) fn error_json(msg: &str, status: u16) -> Result<Response> {
    Ok(Response::from_json(&json!({ "error": msg }))?.with_status(status))
}

/// Cloudflare Access sets this header after authenticating a request. If it
/// is absent, either Access is not configured for this route yet or the
/// request bypassed it; refuse to serve writes either way.
fn require_access(req: &Request) -> Option<Result<Response>> {
    match req.headers().get("cf-access-jwt-assertion").ok().flatten() {
        Some(_) => None,
        None => Some(error_json("Cloudflare Access required", 403)),
    }
}

fn query_param(req: &Request, key: &str) -> Option<String> {
    req.url().ok().and_then(|u| {
        u.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    })
}

async fn home(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let q = query_param(&req, "q").unwrap_or_default();
    let tins = db::list_tins(&ctx.env.d1("DB")?, &q).await?;
    Response::from_html(html::home(&tins, &q))
}

async fn getting_started(_req: Request, _ctx: RouteContext<()>) -> Result<Response> {
    Response::from_html(html::getting_started())
}

async fn install_modes(_req: Request, _ctx: RouteContext<()>) -> Result<Response> {
    Response::from_html(html::install_modes())
}

async fn packaging(_req: Request, _ctx: RouteContext<()>) -> Result<Response> {
    Response::from_html(html::packaging())
}

async fn tin_page(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let name = ctx.param("name").expect("route param");
    match db::tin_detail(&ctx.env.d1("DB")?, name).await? {
        Some(detail) => Response::from_html(html::tin(&detail)),
        None => Response::error("tin not found", 404),
    }
}

async fn api_list(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let q = query_param(&req, "q").unwrap_or_default();
    let tins = db::list_tins(&ctx.env.d1("DB")?, &q).await?;
    Response::from_json(&tins)
}

async fn api_tin(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let name = ctx.param("name").expect("route param");
    match db::tin_detail(&ctx.env.d1("DB")?, name).await? {
        Some(detail) => Response::from_json(&detail),
        None => error_json("tin not found", 404),
    }
}

/// Walks the dependency graph breadth-first, pinning the requested version
/// for the root and the latest published version for every dependency.
async fn api_resolve(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let name = ctx.param("name").expect("route param").clone();
    let want = query_param(&req, "version");
    let d1 = ctx.env.d1("DB")?;

    let mut out: Vec<ResolvedTin> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, Option<String>)> = VecDeque::new();
    queue.push_back((name, want));

    while let Some((tin_name, want)) = queue.pop_front() {
        if !visited.insert(tin_name.clone()) {
            continue;
        }
        let Some(tin) = db::tin_by_name(&d1, &tin_name).await? else {
            return error_json(&format!("tin '{tin_name}' not found"), 404);
        };
        let versions = db::versions_of(&d1, tin.id).await?;
        let chosen = match &want {
            Some(v) => versions.iter().find(|row| &row.version == v),
            None => shelf_core::latest_version(versions.iter().map(|r| r.version.as_str()))
                .and_then(|latest| versions.iter().find(|r| r.version == latest)),
        };
        let Some(chosen) = chosen else {
            let msg = match want {
                Some(v) => format!("tin '{tin_name}' has no version '{v}'"),
                None => format!("tin '{tin_name}' has no published versions"),
            };
            return error_json(&msg, 404);
        };
        for dep in db::dependency_names(&d1, chosen.id).await? {
            queue.push_back((dep, None));
        }
        out.push(ResolvedTin {
            name: tin.name,
            url: tin.url,
            version: chosen.version.clone(),
            commit_sha: chosen.commit_sha.clone(),
        });
    }
    Response::from_json(&out)
}

async fn api_publish(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let d1 = ctx.env.d1("DB")?;
    let Some(author) = authors::bearer_author(&req, &d1).await? else {
        return error_json(
            "invalid or missing publish token; get one at https://mojoshelf.org/authors",
            401,
        );
    };
    let Ok(body) = req.json::<PublishRequest>().await else {
        return error_json("invalid JSON body", 400);
    };
    if body.name.is_empty()
        || !body.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return error_json("name must be non-empty [a-z0-9_-]", 400);
    }
    if semver::Version::parse(&body.version).is_err() {
        return error_json(&format!("'{}' is not a valid semver version", body.version), 400);
    }
    if !is_full_sha(&body.commit_sha) {
        return error_json("commit_sha must be a full 40-character sha", 400);
    }
    if body.url.is_empty() {
        return error_json("url is required", 400);
    }

    let description = body.description.as_deref().map(str::trim).filter(|d| !d.is_empty());
    let tags = shelf_core::split_tags(&body.tags.join(",")).join(",");
    let tin = match db::tin_by_name(&d1, &body.name).await? {
        Some(existing) => {
            if existing.author_id.is_some() && existing.author_id != Some(author.id) {
                return error_json(
                    &format!("tin '{}' is owned by another author", body.name),
                    403,
                );
            }
            db::claim_tin(&d1, existing.id, &body.url, author.id, description, &tags).await?;
            existing
        }
        None => {
            db::create_tin(&d1, &body.name, &body.url, author.id, description, &tags).await?;
            db::tin_by_name(&d1, &body.name)
                .await?
                .ok_or_else(|| worker::Error::RustError("tin vanished after insert".into()))?
        }
    };
    let versions = db::versions_of(&d1, tin.id).await?;
    if versions.iter().any(|v| v.version == body.version) {
        return error_json(
            &format!("version {} of '{}' is already published", body.version, body.name),
            409,
        );
    }
    let mut dep_ids = Vec::new();
    for dep in &body.dependencies {
        match db::tin_by_name(&d1, dep).await? {
            Some(b) => dep_ids.push(b.id),
            None => {
                return error_json(&format!("dependency '{dep}' is not a registered tin"), 400)
            }
        }
    }
    db::insert_version(&d1, tin.id, &body.version, &body.commit_sha, &dep_ids).await?;
    Ok(Response::from_json(&json!({ "ok": true }))?.with_status(201))
}

async fn admin_page(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    if let Some(denied) = require_access(&req) {
        return denied;
    }
    let email = req
        .headers()
        .get("cf-access-authenticated-user-email")
        .ok()
        .flatten()
        .unwrap_or_else(|| "unknown".into());
    let tins = db::list_tins(&ctx.env.d1("DB")?, "").await?;
    Response::from_html(html::admin(&tins, &email))
}

async fn admin_upsert(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    if let Some(denied) = require_access(&req) {
        return denied;
    }
    let form = req.form_data().await?;
    let field = |key: &str| match form.get(key) {
        Some(FormEntry::Field(v)) => v.trim().to_string(),
        _ => String::new(),
    };
    let (name, url, description) = (field("name"), field("url"), field("description"));
    if name.is_empty() || url.is_empty() {
        return error_json("name and url are required", 400);
    }
    db::upsert_tin(&ctx.env.d1("DB")?, &name, &url, &description).await?;
    let mut back = req.url()?;
    back.set_path("/admin");
    back.set_query(None);
    Response::redirect_with_status(back, 303)
}
