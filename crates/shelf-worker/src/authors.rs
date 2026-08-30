//! GitHub sign-in and the author dashboard (publish tokens, deleting
//! versions/tins).

use crate::{auth, db, error_json, html};
use serde::Deserialize;
use serde_json::json;
use crate::located::Located;
use worker::wasm_bindgen::JsValue;
use worker::*;

fn now_ms() -> u64 {
    Date::now().as_millis()
}

fn see_other(location: &str, cookies: &[String]) -> Result<Response> {
    let headers = Headers::new();
    headers.set("Location", location)?;
    for c in cookies {
        headers.append("Set-Cookie", c)?;
    }
    Ok(Response::empty()?.with_status(303).with_headers(headers))
}

fn session_secret(env: &Env) -> Option<String> {
    env.secret("SESSION_SECRET").ok().map(|s| s.to_string())
}

pub async fn current_author(req: &Request, ctx: &RouteContext<()>) -> Result<Option<db::AuthorRow>> {
    let Some(secret) = session_secret(&ctx.env) else {
        return Ok(None);
    };
    let Some(cookie) = auth::cookie(req, auth::SESSION_COOKIE) else {
        return Ok(None);
    };
    let Some(author_id) = auth::verify_session(&secret, &cookie, now_ms()) else {
        return Ok(None);
    };
    db::author_by_id(&ctx.env.d1("DB")?, author_id).await
}

/// The author authenticated by the `Authorization: Bearer` publish token.
pub async fn bearer_author(req: &Request, d1: &D1Database) -> Result<Option<db::AuthorRow>> {
    let Some(token) = auth::bearer_token(req) else {
        return Ok(None);
    };
    db::author_by_token_hash(d1, &auth::sha256_hex(&token)).await
}

// --- GitHub OAuth ---

pub async fn login(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let Ok(client_id) = ctx.env.var("GITHUB_CLIENT_ID") else {
        return error_json("GitHub sign-in is not configured yet", 503);
    };
    let state = auth::random_hex(16)?;
    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={client_id}\
         &redirect_uri=https%3A%2F%2Fmojoshelf.org%2Fauth%2Fcallback&state={state}"
    );
    see_other(&url, &[auth::set_cookie(auth::STATE_COOKIE, &state, 600)])
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: Option<String>,
}

#[derive(Deserialize)]
struct GhUser {
    id: i64,
    login: String,
}

async fn github_user(env: &Env, code: &str) -> Result<Option<GhUser>> {
    let client_id = env.var("GITHUB_CLIENT_ID")?.to_string();
    let client_secret = env.secret("GITHUB_CLIENT_SECRET")?.to_string();

    let headers = Headers::new();
    headers.set("Accept", "application/json")?;
    headers.set("Content-Type", "application/json")?;
    headers.set("User-Agent", "mojoshelf")?;
    let body = json!({ "client_id": client_id, "client_secret": client_secret, "code": code });
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(&body.to_string())));
    let req = Request::new_with_init("https://github.com/login/oauth/access_token", &init)?;
    let token: TokenResp = Fetch::Request(req).send().await.at()?.json().await.at()?;
    let Some(access_token) = token.access_token else {
        return Ok(None);
    };

    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {access_token}"))?;
    headers.set("Accept", "application/vnd.github+json")?;
    headers.set("User-Agent", "mojoshelf")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(headers);
    let req = Request::new_with_init("https://api.github.com/user", &init)?;
    Ok(Some(Fetch::Request(req).send().await.at()?.json().await.at()?))
}

pub async fn callback(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let url = req.url()?;
    let get = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    let (Some(code), Some(state)) = (get("code"), get("state")) else {
        return error_json("missing code/state", 400);
    };
    if auth::cookie(&req, auth::STATE_COOKIE) != Some(state) {
        return error_json("OAuth state mismatch; try signing in again", 400);
    }
    let Some(user) = github_user(&ctx.env, &code).await.at()? else {
        return error_json("GitHub did not accept the sign-in; try again", 400);
    };
    let author = db::upsert_author(&ctx.env.d1("DB")?, user.id, &user.login).await.at()?;
    let Some(secret) = session_secret(&ctx.env) else {
        return error_json("session secret is not configured", 503);
    };
    let session = auth::session_value(&secret, author.id, now_ms());
    see_other(
        "/authors",
        &[
            auth::set_cookie(auth::SESSION_COOKIE, &session, 30 * 24 * 3600),
            auth::clear_cookie(auth::STATE_COOKIE),
        ],
    )
}

pub async fn logout(_req: Request, _ctx: RouteContext<()>) -> Result<Response> {
    see_other("/authors", &[auth::clear_cookie(auth::SESSION_COOKIE)])
}

// --- Dashboard ---

pub async fn page(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let d1 = ctx.env.d1("DB")?;
    match current_author(&req, &ctx).await.at()? {
        None => Response::from_html(html::authors_signed_out()),
        Some(author) => {
            let tins = author_tins_with_versions(&d1, author.id).await.at()?;
            Response::from_html(html::authors_dashboard(
                &author.github_login,
                author.token_hash.is_some(),
                &tins,
                None,
            ))
        }
    }
}

async fn author_tins_with_versions(
    d1: &D1Database,
    author_id: i64,
) -> Result<Vec<(db::TinRow, Vec<db::VersionRow>)>> {
    let mut out = Vec::new();
    for tin in db::tins_of_author(d1, author_id).await.at()? {
        let versions = db::versions_of(d1, tin.id).await.at()?;
        out.push((tin, versions));
    }
    Ok(out)
}

/// Generates (or rotates) the author's publish token; shown once.
pub async fn generate_token(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let Some(author) = current_author(&req, &ctx).await.at()? else {
        return error_json("sign in first", 401);
    };
    let d1 = ctx.env.d1("DB")?;
    let token = format!("shelf_{}", auth::random_hex(24)?);
    db::set_token_hash(&d1, author.id, &auth::sha256_hex(&token)).await.at()?;
    let tins = author_tins_with_versions(&d1, author.id).await.at()?;
    Response::from_html(html::authors_dashboard(
        &author.github_login,
        true,
        &tins,
        Some(&token),
    ))
}

async fn owned_tin(
    req: &Request,
    ctx: &RouteContext<()>,
) -> Result<std::result::Result<(db::AuthorRow, db::TinRow), Response>> {
    let Some(author) = current_author(req, ctx).await.at()? else {
        return Ok(Err(error_json("sign in first", 401)?));
    };
    let name = ctx.param("name").expect("route param");
    let Some(tin) = db::tin_by_name(&ctx.env.d1("DB")?, name).await.at()? else {
        return Ok(Err(error_json("tin not found", 404)?));
    };
    if tin.author_id != Some(author.id) {
        return Ok(Err(error_json("you do not own this tin", 403)?));
    }
    Ok(Ok((author, tin)))
}

/// Public page listing all tins published by one author.
pub async fn author_page(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let login = ctx.param("login").expect("route param");
    let d1 = ctx.env.d1("DB")?;
    if db::author_by_login(&d1, login).await.at()?.is_none() {
        return Response::error("author not found", 404);
    }
    let tins: Vec<_> = db::list_tins(&d1, "", -1, 0)
        .await.at()?
        .into_iter()
        .filter(|b| b.author.as_deref() == Some(login.as_str()))
        .collect();
    Response::from_html(html::author(login, &tins))
}

pub async fn delete_tin(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let (_, tin) = match owned_tin(&req, &ctx).await.at()? {
        Ok(v) => v,
        Err(resp) => return Ok(resp),
    };
    let d1 = ctx.env.d1("DB")?;
    if db::dependent_count(&d1, tin.id).await.at()? > 0 {
        return error_json("other tins depend on this tin; it cannot be deleted", 409);
    }
    db::delete_tin(&d1, tin.id).await.at()?;
    see_other("/authors", &[])
}

pub async fn delete_version(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let (_, tin) = match owned_tin(&req, &ctx).await.at()? {
        Ok(v) => v,
        Err(resp) => return Ok(resp),
    };
    let version = ctx.param("version").expect("route param");
    let d1 = ctx.env.d1("DB")?;
    let versions = db::versions_of(&d1, tin.id).await.at()?;
    let Some(row) = versions.iter().find(|v| &v.version == version) else {
        return error_json("version not found", 404);
    };
    if versions.len() == 1 && db::dependent_count(&d1, tin.id).await.at()? > 0 {
        return error_json(
            "this is the last published version and other tins depend on this tin",
            409,
        );
    }
    db::delete_version(&d1, row.id).await.at()?;
    see_other("/authors", &[])
}
