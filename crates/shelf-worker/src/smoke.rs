//! Asking tin-smoke to verify a tin right after it is published.
//!
//! Verification badges come from the `tin-smoke` workflow, which sweeps the
//! whole registry weekly. A tin published on Monday afternoon therefore wore a
//! grey "not verified" badge for six days. Publishing now dispatches a run
//! scoped to that one tin, so the badge follows publication instead.
//!
//! Best effort throughout: a publish is a user's write and must not fail
//! because a workflow could not be triggered.

use worker::wasm_bindgen::JsValue;
use worker::*;

/// The repository holding the workflow, i.e. this one.
const SMOKE_REPO: &str = "mojoshelf/mojoshelf";
const SMOKE_WORKFLOW: &str = "tin-smoke.yml";

/// Dispatches a `tin-smoke` run for `name`, at most once a day per tin.
///
/// A version bump usually means one publish, but a burst of them should not
/// mean a burst of full smoke matrices, hence the daily limit.
pub async fn request(env: &Env, d1: &D1Database, name: &str) {
    match crate::db::smoke_due(d1, name).await {
        Ok(true) => {}
        // Already asked today: nothing to do, and nothing worth reporting.
        Ok(false) => return,
        Err(e) => {
            console_log!("smoke: could not check the daily limit for {name}: {e}");
            return;
        }
    }
    match dispatch(env, name).await {
        Ok(true) => {
            // Stamped only once the run is actually queued, so a failed
            // dispatch is retried by the next publish rather than suppressed
            // for a day.
            if let Err(e) = crate::db::mark_smoke_requested(d1, name).await {
                console_log!("smoke: dispatched {name} but could not stamp it: {e}");
            } else {
                console_log!("smoke: requested verification of {name}");
            }
        }
        // No token configured: expected in dev, and not an error.
        Ok(false) => console_log!("smoke: no GITHUB_DISPATCH_TOKEN, skipping {name}"),
        Err(e) => {
            let raw = e.to_string();
            let (message, location) = crate::located::split(&raw);
            crate::posthog_exception(
                "SmokeDispatchError",
                message.to_string(),
                location.map(str::to_string),
                serde_json::json!({ "tin": name }),
            )
            .await;
            console_log!("smoke: dispatching {name} failed: {message}");
        }
    }
}

/// `Ok(false)` when no dispatch token is configured; `Ok(true)` once GitHub
/// has accepted the run.
async fn dispatch(env: &Env, name: &str) -> Result<bool> {
    // Deliberately not GITHUB_TOKEN: that one only needs to read public repos,
    // while this needs actions:write on this repository. Keeping them apart
    // means the widely-used read token stays powerless.
    let Ok(token) = env.secret("GITHUB_DISPATCH_TOKEN") else {
        return Ok(false);
    };
    let headers = Headers::new();
    headers.set("User-Agent", "mojoshelf-sync")?;
    headers.set("Accept", "application/vnd.github+json")?;
    headers.set("Content-Type", "application/json")?;
    headers.set("Authorization", &format!("Bearer {}", token.to_string()))?;
    let body = serde_json::json!({
        "ref": "main",
        "inputs": { "only": name },
    });
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(&body.to_string())));
    let url = format!(
        "https://api.github.com/repos/{SMOKE_REPO}/actions/workflows/{SMOKE_WORKFLOW}/dispatches"
    );
    let mut res = Fetch::Request(Request::new_with_init(&url, &init)?)
        .send()
        .await?;
    match res.status_code() {
        // The dispatch endpoint answers 204 with an empty body.
        204 => Ok(true),
        status => {
            let detail = res
                .text()
                .await
                .map(|b| b.chars().take(160).collect::<String>())
                .unwrap_or_default();
            Err(Error::RustError(format!(
                "workflow dispatch returned {status}: {detail}"
            )))
        }
    }
}
