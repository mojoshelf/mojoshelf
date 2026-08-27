//! The mojoshelf registry Worker: public JSON API, public index page, and
//! admin pages. Admin and publish routes are gated by Cloudflare Access at
//! the edge; the in-Worker header check below is defense in depth only.

mod auth;
mod authors;
mod channel;
mod db;
mod html;
mod mcp;

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
        .get_async("/community-channel", community_channel)
        .get_async("/tins/:name", tin_page)
        .get_async("/llms.txt", llms_txt)
        .get_async("/llms-full.txt", llms_full)
        .get_async("/mcp", mcp::get)
        .post_async("/mcp", mcp::post)
        .options_async("/mcp", mcp::options)
        .get_async("/api/tins", api_list)
        .get_async("/api/tins/:name", api_tin)
        .get_async("/api/tins/:name/resolve", api_resolve)
        .get_async("/api/tins/:name/card", api_tin_card)
        .post_async("/api/publish", api_publish)
        .post_async("/api/verify", api_verify)
        .post_async("/api/sync-channel", api_sync_channel)
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

#[event(scheduled)]
pub async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    match channel::sync(&env).await {
        Ok(msg) => console_log!("channel sync: {msg}"),
        Err(e) => console_log!("channel sync FAILED: {e}"),
    }
}

/// Manual channel sync, gated by any valid publish token (idempotent).
async fn api_sync_channel(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let d1 = ctx.env.d1("DB")?;
    if authors::bearer_author(&req, &d1).await?.is_none() {
        return error_json("publish token required", 401);
    }
    let msg = channel::sync(&ctx.env).await?;
    Response::from_json(&json!({ "ok": true, "result": msg }))
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

async fn community_channel(_req: Request, _ctx: RouteContext<()>) -> Result<Response> {
    Response::from_html(html::community_channel())
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
        if tin.kind == "channel" {
            // Binary channel package: the conda solver owns it (and its
            // dependency graph) — a single unpinned entry.
            out.push(ResolvedTin {
                version: tin.channel_version.clone().unwrap_or_default(),
                name: tin.name,
                url: tin.url,
                commit_sha: String::new(),
                kind: "channel".into(),
                prev_url: tin.prev_url,
                url_changed_at: tin.url_changed_at,
            });
            continue;
        }
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
            kind: "source".into(),
            prev_url: tin.prev_url,
            url_changed_at: tin.url_changed_at,
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
            if existing.kind == "channel" {
                return error_json(
                    &format!(
                        "'{}' is a modular-community channel package; pick a \
                         different tin name",
                        body.name
                    ),
                    409,
                );
            }
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

fn text_response(body: String, content_type: &str) -> Result<Response> {
    let headers = Headers::new();
    headers.set("Content-Type", content_type)?;
    Ok(Response::ok(body)?.with_headers(headers))
}

/// llms.txt: a compact machine-readable index for agents and crawlers —
/// what mojoshelf is, how to install, one line per tin.
async fn llms_txt(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let tins = db::list_tins(&ctx.env.d1("DB")?, "").await?;
    let mut out = String::from(
        "# mojoshelf\n\n\
         > An experimental community registry of reusable Mojo libraries (\"tins\"), \
         installed as registry-pinned pixi git source dependencies or git submodules. \
         Includes a read-only mirror of the modular-community conda channel (packages \
         marked \"channel binary\"). Not affiliated with Modular.\n\n\
         Install the CLI: `pixi global install --channel https://mojoshelf.org/channel mojoshelf`\n\
         Then `pixi shelf add <name>` (pixi mode) or `shelf add <name>` (submodule mode).\n\n\
         JSON API: `/api/tins?q=<term>` (search), `/api/tins/<name>` (detail), \
         `/api/tins/<name>/resolve` (pinned install set). \
         Markdown per tin: `/api/tins/<name>/card`.\n\
         All tin cards in one file: https://mojoshelf.org/llms-full.txt\n\n\
         ## Tins\n\n",
    );
    for t in &tins {
        out.push_str(&format!(
            "- [{name}](https://mojoshelf.org/tins/{name}): {desc}{badge}\n",
            name = t.name,
            desc = t.description.as_deref().unwrap_or("(no description yet)"),
            badge = if t.kind == "channel" { " (channel binary)" } else { "" },
        ));
    }
    text_response(out, "text/plain; charset=utf-8")
}

/// llms-full.txt: every tin card, separated by rules. Tins whose card the
/// cron has not built yet get a minimal stub so the file is complete.
async fn llms_full(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let rows = db::all_cards(&ctx.env.d1("DB")?).await?;
    let mut out = String::from(
        "# mojoshelf — all tin cards\n\n\
         One markdown card per tin (source tins first, then modular-community \
         channel mirrors). Index: https://mojoshelf.org/llms.txt\n",
    );
    for (name, description, url, card) in rows {
        out.push_str("\n---\n\n");
        match card {
            Some(c) => out.push_str(&c),
            None => out.push_str(&format!(
                "# {name}\n\n{desc}\n\n- repository: {url}\n\
                 - details: https://mojoshelf.org/tins/{name}\n",
                desc = description.as_deref().unwrap_or("(no description yet)"),
            )),
        }
    }
    text_response(out, "text/markdown; charset=utf-8")
}

/// A tin's card: the stored one, or — while the cron hasn't reached the tin
/// yet — a metadata-only card assembled on the fly (no repo fetches).
/// None = no such tin. Shared by the REST card endpoint and the MCP tools.
pub(crate) async fn card_markdown(d1: &D1Database, name: &str) -> Result<Option<String>> {
    match db::card_of(d1, name).await? {
        None => Ok(None),
        Some(Some(card)) => Ok(Some(card)),
        Some(None) => Ok(db::tin_detail(d1, name)
            .await?
            .map(|detail| shelf_core::cards::assemble_card(&detail, &Default::default()))),
    }
}

async fn api_tin_card(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let name = ctx.param("name").expect("route param");
    match card_markdown(&ctx.env.d1("DB")?, name).await? {
        Some(card) => text_response(card, "text/markdown; charset=utf-8"),
        None => error_json("tin not found", 404),
    }
}

/// Body of POST /api/verify: tin-smoke build outcomes reported by CI.
#[derive(serde::Deserialize)]
struct VerifyBody {
    results: Vec<VerifyResult>,
}
#[derive(serde::Deserialize)]
struct VerifyResult {
    name: String,
    ok: bool,
    #[serde(default)]
    compiler: Option<String>,
}

async fn api_verify(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let d1 = ctx.env.d1("DB")?;
    if authors::bearer_author(&req, &d1).await?.is_none() {
        return error_json("publish token required", 401);
    }
    let Ok(body) = req.json::<VerifyBody>().await else {
        return error_json("invalid JSON body; expected {\"results\": [{\"name\", \"ok\", \"compiler\"?}]}", 400);
    };
    let (mut updated, mut unknown) = (0usize, 0usize);
    for r in &body.results {
        match db::tin_by_name(&d1, &r.name).await? {
            Some(_) => {
                db::set_verified(&d1, &r.name, r.ok, r.compiler.as_deref()).await?;
                updated += 1;
            }
            None => unknown += 1,
        }
    }
    Response::from_json(&json!({ "ok": true, "updated": updated, "unknown": unknown }))
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
