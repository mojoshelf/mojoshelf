//! The mojoshelf registry Worker: public JSON API, public index page, and
//! admin pages. Admin and publish routes are gated by Cloudflare Access at
//! the edge; the in-Worker header check below is defense in depth only.

mod auth;
mod authors;
mod badge;
mod channel;
mod db;
mod html;
mod located;
mod manifest;
mod mcp;
mod smoke;

use crate::located::{split as split_location, Located};
use serde_json::json;
use shelf_core::{is_full_sha, PublishRequest, ResolvedTin};
use std::collections::{HashMap, HashSet, VecDeque};
use worker::*;

#[event(fetch)]
pub async fn main(req: Request, env: Env, ctx: Context) -> Result<Response> {
    let tracked = server_event(&req).await;
    // Kept for the exception below: the router consumes `req`.
    let route = format!("{} {}", String::from(req.method()), req.path());
    let resp = Router::new()
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
        .get_async("/ph/*path", posthog_proxy)
        .post_async("/ph/*path", posthog_proxy)
        .options_async("/ph/*path", posthog_proxy)
        .get_async("/api/tins", api_list)
        .get_async("/api/tins/:name", api_tin)
        .get_async("/api/tins/:name/resolve", api_resolve)
        .get_async("/api/tins/:name/card", api_tin_card)
        .get_async("/api/tins/:name/badge", api_tin_badge)
        .get_async("/badge/:file", badge_stable_svg)
        .get_async("/badge/:name/nightly.svg", badge_nightly_svg)
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
        .await;
    // Every handler returns `Result<Response>`, so a bubbled-up `Err` is the
    // one place an unhandled failure turns into a 500. Report it before
    // propagating, which the `?` here used to skip.
    let resp = match resp {
        Ok(resp) => resp,
        Err(e) => {
            // `.at()` recorded where the `?` failed; take it back off so the
            // location is reported to PostHog but never reaches the client.
            let raw = e.to_string();
            let (message, location) = split_location(&raw);
            ctx.wait_until(posthog_exception(
                "WorkerError",
                message.to_string(),
                location.map(str::to_string),
                json!({ "route": route }),
            ));
            return Err(Error::RustError(message.to_string()));
        }
    };
    if let Some((event, distinct_id, mut props)) = tracked {
        props["status"] = json!(resp.status_code());
        ctx.wait_until(posthog_capture(event, distinct_id, props));
    }
    Ok(resp)
}

/// Server-side analytics for agent traffic: MCP calls and /api/tins hits.
/// Anonymous — the distinct id is a truncated hash of ip+user-agent and no
/// person profiles are created. Returns (event, distinct_id, properties).
async fn server_event(req: &Request) -> Option<(&'static str, String, serde_json::Value)> {
    let path = req.path();
    let (event, mut props) = if path == "/mcp" && req.method() == Method::Post {
        let mut clone = req.clone().ok()?;
        let body: serde_json::Value = clone.json().await.ok()?;
        let rpc = body.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let tool = body.pointer("/params/name").and_then(|t| t.as_str());
        ("mcp_request", json!({ "rpc_method": rpc, "tool": tool }))
    } else if path.starts_with("/api/tins") && req.method() == Method::Get {
        let rest = path.trim_start_matches("/api/tins").trim_start_matches('/');
        let mut parts = rest.split('/');
        let tin = parts.next().unwrap_or("");
        let endpoint = match (tin.is_empty(), parts.next()) {
            (true, _) => "list",
            (false, None) => "detail",
            (false, Some(sub)) => match sub {
                "resolve" => "resolve",
                "card" => "card",
                _ => "other",
            },
        };
        let tin = (!tin.is_empty()).then(|| tin.to_string());
        (
            "api_tins_request",
            json!({ "endpoint": endpoint, "tin": tin }),
        )
    } else {
        return None;
    };
    let ip = req
        .headers()
        .get("cf-connecting-ip")
        .ok()
        .flatten()
        .unwrap_or_default();
    let ua = req
        .headers()
        .get("user-agent")
        .ok()
        .flatten()
        .unwrap_or_default();
    props["path"] = json!(path);
    props["user_agent"] = json!(ua);
    props["$process_person_profile"] = json!(false);
    let distinct_id = format!("agent-{}", &auth::sha256_hex(&format!("{ip}|{ua}"))[..16]);
    Some((event, distinct_id, props))
}

async fn posthog_capture(event: &'static str, distinct_id: String, properties: serde_json::Value) {
    let payload = json!({
        "api_key": html::POSTHOG_KEY,
        "event": event,
        "distinct_id": distinct_id,
        "properties": properties,
    });
    let headers = Headers::new();
    let _ = headers.set("content-type", "application/json");
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(wasm_bindgen::JsValue::from_str(&payload.to_string())));
    if let Ok(r) = Request::new_with_init("https://us.i.posthog.com/i/v0/e/", &init) {
        let _ = Fetch::Request(r).send().await;
    }
}

/// Reports a Rust error to PostHog Error Tracking as an `$exception` event.
///
/// There is no stack trace: this is a wasm Worker, so the frames the Rust SDK
/// would walk (via `backtrace`/`findshlibs`) do not exist. `kind` is therefore
/// what issues group by — keep it coarse and stable — and `context` carries
/// the locator, such as the route that failed.
pub(crate) async fn posthog_exception(
    kind: &'static str,
    message: String,
    location: Option<String>,
    context: serde_json::Value,
) {
    // One frame, from `#[track_caller]` rather than an unwinder, so it is
    // already resolved: PostHog symbolicates nothing and shows it as-is.
    let stacktrace = location.as_deref().and_then(|loc| {
        let (file, line) = loc.rsplit_once(':')?;
        json!({
            "type": "raw",
            "frames": [{
                "filename": file,
                "lineno": line.parse::<u32>().ok()?,
                "function": "",
                "lang": "rust",
                "platform": "native",
                "in_app": true,
                "synthetic": false,
                "client_resolved": true,
            }],
        })
        .into()
    });
    let mut props = json!({
        "$process_person_profile": false,
        "$exception_level": "error",
        "$exception_list": [{
            "type": kind,
            "value": message,
            "mechanism": { "type": "generic", "handled": false, "synthetic": stacktrace.is_none() },
            "stacktrace": stacktrace,
        }],
        "runtime": "cloudflare-workers",
    });
    if let (serde_json::Value::Object(props), serde_json::Value::Object(extra)) =
        (&mut props, context)
    {
        props.extend(extra);
    }
    // Personless, so the id only has to be unique per report.
    let distinct_id = format!(
        "worker-{}",
        auth::sha256_hex(&Date::now().as_millis().to_string())
    );
    posthog_capture("$exception", distinct_id, props).await;
}

#[event(scheduled)]
pub async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    match channel::sync(&env).await {
        Ok(msg) => console_log!("channel sync: {msg}"),
        Err(e) => {
            console_log!("channel sync FAILED: {e}");
            // The cron has no response to fail, so a broken mirror sync is
            // otherwise invisible outside the logs.
            let raw = e.to_string();
            let (message, location) = split_location(&raw);
            posthog_exception(
                "ChannelSyncError",
                message.to_string(),
                location.map(str::to_string),
                json!({ "job": "channel-sync" }),
            )
            .await;
        }
    }
}

/// GET /badge/<tin>.svg — stable verification badge.
async fn badge_stable_svg(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let file = ctx.param("file").expect("route param").clone();
    let Some(name) = file.strip_suffix(".svg") else {
        return Ok(Response::error("expected /badge/<tin>.svg", 404)?);
    };
    serve_badge(&ctx, name, false).await
}

/// GET /badge/<tin>/nightly.svg — nightly early-warning badge.
async fn badge_nightly_svg(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let name = ctx.param("name").expect("route param").clone();
    serve_badge(&ctx, &name, true).await
}

async fn serve_badge(ctx: &RouteContext<()>, name: &str, nightly: bool) -> Result<Response> {
    let d1 = ctx.env.d1("DB")?;
    let Some(tin) = db::tin_by_name(&d1, name).await.at()? else {
        return Ok(Response::error("unknown tin", 404)?);
    };
    let (label, (message, color)) = if nightly {
        (
            "mojo nightly",
            badge::nightly_state(
                tin.nightly_ok.map(|v| v != 0),
                tin.nightly_compiler.as_deref(),
            ),
        )
    } else {
        (
            "mojoshelf",
            badge::stable_state(
                tin.verified_ok.map(|v| v != 0),
                tin.verified_compiler.as_deref(),
            ),
        )
    };
    let headers = Headers::new();
    headers.set("content-type", "image/svg+xml; charset=utf-8")?;
    headers.set("cache-control", "public, max-age=3600")?;
    Ok(Response::ok(badge::render(label, &message, color))?.with_headers(headers))
}

/// GET /api/tins/:name/badge[?channel=nightly] — same data in the
/// shields.io endpoint schema, for repos that prefer shields styling.
async fn api_tin_badge(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let name = ctx.param("name").expect("route param").clone();
    let nightly = req
        .url()?
        .query_pairs()
        .any(|(k, v)| k == "channel" && v == "nightly");
    let d1 = ctx.env.d1("DB")?;
    let Some(tin) = db::tin_by_name(&d1, &name).await.at()? else {
        return error_json("tin not found", 404);
    };
    let (label, (message, color)) = if nightly {
        (
            "mojo nightly",
            badge::nightly_state(
                tin.nightly_ok.map(|v| v != 0),
                tin.nightly_compiler.as_deref(),
            ),
        )
    } else {
        (
            "mojoshelf",
            badge::stable_state(
                tin.verified_ok.map(|v| v != 0),
                tin.verified_compiler.as_deref(),
            ),
        )
    };
    Response::from_json(&json!({
        "schemaVersion": 1,
        "label": label,
        "message": message,
        "color": badge::shields_color(color),
    }))
}

/// Reverse proxy for PostHog so analytics requests stay first-party
/// (ad-blockers drop *.posthog.com). `/ph/static/*` goes to the assets
/// host, everything else to US ingestion. Cookies never leave our domain;
/// the client IP is forwarded so events geolocate correctly.
async fn posthog_proxy(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let path = ctx.param("path").expect("route param").clone();
    let upstream = if path.starts_with("static/") {
        format!("https://us-assets.i.posthog.com/{path}")
    } else {
        format!("https://us.i.posthog.com/{path}")
    };
    let target = match req.url()?.query() {
        Some(q) => format!("{upstream}?{q}"),
        None => upstream,
    };
    let headers = req.headers().clone();
    headers.delete("cookie")?;
    if let Some(ip) = req.headers().get("cf-connecting-ip")? {
        headers.set("x-forwarded-for", &ip)?;
    }
    let mut init = RequestInit::new();
    init.with_method(req.method()).with_headers(headers);
    if !matches!(req.method(), Method::Get | Method::Head) {
        let body = req.bytes().await.at()?;
        init.with_body(Some(js_sys::Uint8Array::from(body.as_slice()).into()));
    }
    Fetch::Request(Request::new_with_init(&target, &init)?)
        .send()
        .await
}

/// Manual channel sync, gated by any valid publish token (idempotent).
async fn api_sync_channel(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let d1 = ctx.env.d1("DB")?;
    if authors::bearer_author(&req, &d1).await.at()?.is_none() {
        return error_json("publish token required", 401);
    }
    let msg = channel::sync(&ctx.env).await.at()?;
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

/// Tins per page on the public list.
const PAGE_SIZE: i64 = 20;

async fn home(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let q = query_param(&req, "q").unwrap_or_default();
    let d1 = ctx.env.d1("DB")?;
    let total = db::count_tins(&d1, &q).await.at()?;
    // Clamp rather than 404: a stale or hand-edited ?page= lands on the last
    // page instead of an error.
    let last = ((total - 1).max(0) / PAGE_SIZE) + 1;
    let page = query_param(&req, "page")
        .and_then(|p| p.parse::<i64>().ok())
        .unwrap_or(1)
        .clamp(1, last);
    let tins = db::list_tins(&d1, &q, PAGE_SIZE, (page - 1) * PAGE_SIZE)
        .await
        .at()?;
    Response::from_html(html::home(&tins, page, &q, last, total))
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
    match db::tin_detail(&ctx.env.d1("DB")?, name).await.at()? {
        Some(detail) => Response::from_html(html::tin(&detail)),
        None => Response::error("tin not found", 404),
    }
}

async fn api_list(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let q = query_param(&req, "q").unwrap_or_default();
    let tins = db::list_tins(&ctx.env.d1("DB")?, &q, -1, 0).await.at()?;
    Response::from_json(&tins)
}

async fn api_tin(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let name = ctx.param("name").expect("route param");
    match db::tin_detail(&ctx.env.d1("DB")?, name).await.at()? {
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

    // Collected first, then ordered dependencies-first below.
    let mut found: HashMap<String, ResolvedTin> = HashMap::new();
    let mut deps_of: HashMap<String, Vec<String>> = HashMap::new();
    let root = name.clone();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, Option<String>)> = VecDeque::new();
    queue.push_back((name, want));

    while let Some((tin_name, want)) = queue.pop_front() {
        if !visited.insert(tin_name.clone()) {
            continue;
        }
        let Some(tin) = db::tin_by_name(&d1, &tin_name).await.at()? else {
            return error_json(&format!("tin '{tin_name}' not found"), 404);
        };
        if tin.kind == "channel" {
            // Binary channel package: the conda solver owns it (and its
            // dependency graph) — a single unpinned entry.
            found.insert(
                tin.name.clone(),
                ResolvedTin {
                    version: tin.channel_version.clone().unwrap_or_default(),
                    name: tin.name,
                    url: tin.url,
                    commit_sha: String::new(),
                    kind: "channel".into(),
                    prev_url: tin.prev_url,
                    url_changed_at: tin.url_changed_at,
                },
            );
            continue;
        }
        let versions = db::versions_of(&d1, tin.id).await.at()?;
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
        let names = db::dependency_names(&d1, chosen.id).await.at()?;
        for dep in &names {
            queue.push_back((dep.clone(), None));
        }
        deps_of.insert(tin.name.clone(), names);
        found.insert(
            tin.name.clone(),
            ResolvedTin {
                name: tin.name,
                url: tin.url,
                version: chosen.version.clone(),
                commit_sha: chosen.commit_sha.clone(),
                kind: "source".into(),
                prev_url: tin.prev_url,
                url_changed_at: tin.url_changed_at,
            },
        );
    }

    // Dependencies before the tins that need them. `pixi add` solves the
    // environment on every call, so a tin added before its dependencies fails
    // to solve — which is what an install set is for. Submodule mode does not
    // care about order, so ordering here costs it nothing.
    //
    // Post-order depth-first walk from the requested tin. `scheduled` guards
    // the traversal, so a dependency cycle terminates instead of looping.
    let mut out: Vec<ResolvedTin> = Vec::new();
    let mut emitted: HashSet<String> = HashSet::new();
    let mut scheduled: HashSet<String> = HashSet::new();
    let mut stack: Vec<(String, bool)> = vec![(root, false)];
    while let Some((tin_name, expanded)) = stack.pop() {
        if expanded {
            if emitted.insert(tin_name.clone()) {
                if let Some(tin) = found.remove(&tin_name) {
                    out.push(tin);
                }
            }
            continue;
        }
        if !scheduled.insert(tin_name.clone()) {
            continue;
        }
        let deps = deps_of.get(&tin_name).cloned().unwrap_or_default();
        stack.push((tin_name, true));
        for dep in deps {
            if !scheduled.contains(&dep) {
                stack.push((dep, false));
            }
        }
    }
    Response::from_json(&out)
}

async fn api_publish(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let d1 = ctx.env.d1("DB")?;
    let Some(author) = authors::bearer_author(&req, &d1).await.at()? else {
        return error_json(
            "invalid or missing publish token; get one at https://mojoshelf.org/authors",
            401,
        );
    };
    let Ok(body) = req.json::<PublishRequest>().await else {
        return error_json("invalid JSON body", 400);
    };
    if body.name.is_empty()
        || !body
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return error_json("name must be non-empty [a-z0-9_-]", 400);
    }
    if semver::Version::parse(&body.version).is_err() {
        return error_json(
            &format!("'{}' is not a valid semver version", body.version),
            400,
        );
    }
    if !is_full_sha(&body.commit_sha) {
        return error_json("commit_sha must be a full 40-character sha", 400);
    }
    if body.url.is_empty() {
        return error_json("url is required", 400);
    }

    // A tin whose manifest points at sibling checkouts builds for its author
    // and nobody else. Catch it here, where the fix is one edit away, rather
    // than days later through a red verification badge.
    if let Some(text) = manifest::fetch(&body.url, &body.commit_sha).await {
        let escaping = manifest::escaping_path_deps(&text);
        if !escaping.is_empty() {
            let detail = escaping
                .iter()
                .map(|e| format!("{} = {{ path = \"{}\" }} in [{}]", e.name, e.path, e.table))
                .collect::<Vec<_>>()
                .join("; ");
            return error_json(
                &format!(
                    "this version declares dependencies by a path outside the \
                     repository, which only resolves in your own checkout, so \
                     nobody can install it: {detail}. Declare them as git \
                     dependencies pinned to a commit — pixi has no equivalent \
                     of Cargo's path-plus-version fallback, so a published \
                     package has to be self-contained."
                ),
                400,
            );
        }
    }

    let description = body
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());
    let tags = shelf_core::split_tags(&body.tags.join(",")).join(",");
    let tin = match db::tin_by_name(&d1, &body.name).await.at()? {
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
            db::claim_tin(&d1, existing.id, &body.url, author.id, description, &tags)
                .await
                .at()?;
            existing
        }
        None => {
            db::create_tin(&d1, &body.name, &body.url, author.id, description, &tags)
                .await
                .at()?;
            db::tin_by_name(&d1, &body.name)
                .await
                .at()?
                .ok_or_else(|| worker::Error::RustError("tin vanished after insert".into()))?
        }
    };
    let versions = db::versions_of(&d1, tin.id).await.at()?;
    if versions.iter().any(|v| v.version == body.version) {
        return error_json(
            &format!(
                "version {} of '{}' is already published",
                body.version, body.name
            ),
            409,
        );
    }
    let mut dep_ids = Vec::new();
    for dep in &body.dependencies {
        match db::tin_by_name(&d1, dep).await.at()? {
            Some(b) => dep_ids.push(b.id),
            None => return error_json(&format!("dependency '{dep}' is not a registered tin"), 400),
        }
    }
    db::insert_version(&d1, tin.id, &body.version, &body.commit_sha, &dep_ids)
        .await
        .at()?;
    // Verification badges come from the weekly tin-smoke sweep, so without
    // this a tin published just after a sweep looks unverified for a week.
    // Best effort: the publish has already succeeded either way.
    smoke::request(&ctx.env, &d1, &body.name).await;
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
    let tins = db::list_tins(&ctx.env.d1("DB")?, "", -1, 0).await.at()?;
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
            badge = if t.kind == "channel" {
                " (channel binary)"
            } else {
                ""
            },
        ));
    }
    text_response(out, "text/plain; charset=utf-8")
}

/// llms-full.txt: every tin card, separated by rules. Tins whose card the
/// cron has not built yet get a minimal stub so the file is complete.
async fn llms_full(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let rows = db::all_cards(&ctx.env.d1("DB")?).await.at()?;
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
    match db::card_of(d1, name).await.at()? {
        None => Ok(None),
        Some(Some(card)) => Ok(Some(card)),
        Some(None) => Ok(db::tin_detail(d1, name)
            .await
            .at()?
            .map(|detail| shelf_core::cards::assemble_card(&detail, &Default::default()))),
    }
}

async fn api_tin_card(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let name = ctx.param("name").expect("route param");
    match card_markdown(&ctx.env.d1("DB")?, name).await.at()? {
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
    /// "nightly" targets the separate nightly record; anything else (or
    /// absent) is the stable verification.
    #[serde(default)]
    channel: Option<String>,
    /// The tin-smoke run these results came from.
    #[serde(default)]
    run_url: Option<String>,
    /// The error a failing check hit, extracted from the job log.
    #[serde(default)]
    reason: Option<String>,
}

async fn api_verify(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let d1 = ctx.env.d1("DB")?;
    if authors::bearer_author(&req, &d1).await.at()?.is_none() {
        return error_json("publish token required", 401);
    }
    let Ok(body) = req.json::<VerifyBody>().await else {
        return error_json(
            "invalid JSON body; expected {\"results\": [{\"name\", \"ok\", \"compiler\"?}]}",
            400,
        );
    };
    let (mut updated, mut unknown) = (0usize, 0usize);
    for r in &body.results {
        match db::tin_by_name(&d1, &r.name).await.at()? {
            Some(_) => {
                let nightly = r.channel.as_deref() == Some("nightly");
                db::set_verified(
                    &d1,
                    &r.name,
                    r.ok,
                    r.compiler.as_deref(),
                    nightly,
                    r.run_url.as_deref(),
                    r.reason.as_deref(),
                )
                .await
                .at()?;
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
    let tins = db::list_tins(&ctx.env.d1("DB")?, "", -1, 0).await.at()?;
    Response::from_html(html::admin(&tins, &email))
}

async fn admin_upsert(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    if let Some(denied) = require_access(&req) {
        return denied;
    }
    let form = req.form_data().await.at()?;
    let field = |key: &str| match form.get(key) {
        Some(FormEntry::Field(v)) => v.trim().to_string(),
        _ => String::new(),
    };
    let (name, url, description) = (field("name"), field("url"), field("description"));
    if name.is_empty() || url.is_empty() {
        return error_json("name and url are required", 400);
    }
    db::upsert_tin(&ctx.env.d1("DB")?, &name, &url, &description)
        .await
        .at()?;
    let mut back = req.url()?;
    back.set_path("/admin");
    back.set_query(None);
    Response::redirect_with_status(back, 303)
}
