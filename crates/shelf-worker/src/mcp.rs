//! MCP (Model Context Protocol) server at /mcp: stateless streamable HTTP,
//! one JSON-RPC 2.0 request per POST, one JSON response — no sessions, no
//! SSE. Exposes three anonymous read-only tools (search_tins, tin_info,
//! usage_example) over the same D1 queries and precomputed cards the REST
//! API serves.

use crate::db;
use crate::located::Located;
use serde_json::{json, Value};
use shelf_core::TinSummary;
use worker::*;

const LATEST_PROTOCOL: &str = "2025-06-18";
const SUPPORTED_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "mojoshelf is a community registry of reusable Mojo \
    libraries (\"tins\"). Use search_tins to find libraries by topic or name, \
    tin_info for a tin's full card (import name, install commands, API surface, \
    health), and usage_example for install commands plus a copy-pasteable \
    snippet. If search_tins finds nothing relevant, say so — do not guess \
    package names.";

/// Browser-based MCP clients need CORS on every response.
fn with_cors(resp: Response) -> Result<Response> {
    let headers = resp.headers().clone();
    headers.set("Access-Control-Allow-Origin", "*")?;
    headers.set("Access-Control-Allow-Methods", "GET, POST, OPTIONS")?;
    headers.set(
        "Access-Control-Allow-Headers",
        "Content-Type, Authorization, Mcp-Session-Id, Mcp-Protocol-Version",
    )?;
    Ok(resp.with_headers(headers))
}

pub async fn options(_req: Request, _ctx: RouteContext<()>) -> Result<Response> {
    with_cors(Response::empty()?.with_status(204))
}

/// Streamable HTTP lets a server refuse GET (server-initiated streams)
/// with 405; this server is stateless and has nothing to push.
pub async fn get(_req: Request, _ctx: RouteContext<()>) -> Result<Response> {
    with_cors(Response::error(
        "mojoshelf MCP is POST-only (stateless; no server-initiated streams)",
        405,
    )?)
}

fn rpc_result(id: &Value, result: Value) -> Result<Response> {
    with_cors(Response::from_json(
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    )?)
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Result<Response> {
    with_cors(Response::from_json(&json!({
        "jsonrpc": "2.0", "id": id,
        "error": { "code": code, "message": message },
    }))?)
}

pub async fn post(mut req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let Ok(body) = req.json::<Value>().await else {
        return rpc_error(
            &Value::Null,
            -32700,
            "parse error: expected one JSON-RPC 2.0 request object",
        );
    };
    let method = body
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let Some(id) = body.get("id").cloned() else {
        // A notification (notifications/initialized, …) expects no reply.
        return with_cors(Response::empty()?.with_status(202));
    };
    if method.is_empty() {
        return rpc_error(&id, -32600, "invalid request: missing method");
    }
    match method.as_str() {
        "initialize" => rpc_result(&id, initialize_result(&body)),
        "ping" => rpc_result(&id, json!({})),
        "tools/list" => rpc_result(&id, json!({ "tools": tool_definitions() })),
        "tools/call" => match call_tool(&ctx, body.get("params")).await {
            Ok(Ok(result)) => rpc_result(&id, result),
            Ok(Err((code, msg))) => rpc_error(&id, code, &msg),
            Err(e) => rpc_error(&id, -32603, &format!("internal error: {e}")),
        },
        other => rpc_error(&id, -32601, &format!("method not found: {other}")),
    }
}

/// Echo a protocol version we support; otherwise offer our latest.
fn initialize_result(body: &Value) -> Value {
    let requested = body
        .pointer("/params/protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("");
    let protocol = if SUPPORTED_PROTOCOLS.contains(&requested) {
        requested
    } else {
        LATEST_PROTOCOL
    };
    json!({
        "protocolVersion": protocol,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "mojoshelf",
            "title": "Mojo Shelf registry",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "search_tins",
            "description": "Search the mojoshelf registry of reusable Mojo \
                libraries (\"tins\") by name, description, tag, GitHub org, \
                or author. An empty \
                query lists every tin. Returns package name, kind (source tin \
                or modular-community channel binary), latest version, \
                description, tags, and build-verification status. If nothing \
                matches, nothing on the shelf covers the topic.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search term (e.g. \"csv\", \"compression\"); empty lists all tins"
                    }
                }
            }
        },
        {
            "name": "tin_info",
            "description": "Full card for one tin: Mojo import name vs conda \
                package name, install commands for both modes, public API \
                surface, usage snippet, dependencies, activity, and whether \
                the latest consumer smoke build passed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Tin name as returned by search_tins" }
                },
                "required": ["name"]
            }
        },
        {
            "name": "usage_example",
            "description": "Install commands and a copy-pasteable usage \
                snippet for one tin, including the correct Mojo import name.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Tin name as returned by search_tins" }
                },
                "required": ["name"]
            }
        }
    ])
}

async fn call_tool(
    ctx: &RouteContext<()>,
    params: Option<&Value>,
) -> Result<std::result::Result<Value, (i64, String)>> {
    let Some(params) = params else {
        return Ok(Err((
            -32602,
            "tools/call needs params {name, arguments}".into(),
        )));
    };
    let tool = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let str_arg = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_string);
    let d1 = ctx.env.d1("DB")?;
    let result = match tool {
        "search_tins" => {
            let query = str_arg("query").unwrap_or_default();
            let tins = db::list_tins(&d1, &query, -1, 0).await.at()?;
            tool_text(render_search(&tins, &query), false)
        }
        "tin_info" => {
            let Some(name) = str_arg("name") else {
                return Ok(Err((
                    -32602,
                    "tin_info needs arguments: {\"name\": …}".into(),
                )));
            };
            match crate::card_markdown(&d1, &name).await.at()? {
                Some(card) => tool_text(card, false),
                None => tool_text(unknown_tin_text(&name), true),
            }
        }
        "usage_example" => {
            let Some(name) = str_arg("name") else {
                return Ok(Err((
                    -32602,
                    "usage_example needs arguments: {\"name\": …}".into(),
                )));
            };
            match crate::card_markdown(&d1, &name).await.at()? {
                Some(card) => tool_text(usage_sections(&card), false),
                None => tool_text(unknown_tin_text(&name), true),
            }
        }
        other => return Ok(Err((-32602, format!("unknown tool: {other}")))),
    };
    Ok(Ok(result))
}

fn tool_text(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn unknown_tin_text(name: &str) -> String {
    format!(
        "No tin named '{name}' is on the shelf. Use search_tins to find the \
         right name — do not guess package names."
    )
}

/// One line per matching tin; honest emptiness when nothing matches.
fn render_search(tins: &[TinSummary], query: &str) -> String {
    if tins.is_empty() {
        return format!(
            "No tins match \"{query}\" — nothing on the shelf covers this; say \
             so rather than guessing a package name. The full index is at \
             https://mojoshelf.org/llms.txt."
        );
    }
    let mut out = if query.is_empty() {
        format!("All {} tins on the shelf:\n\n", tins.len())
    } else {
        format!("{} tin(s) matching \"{query}\":\n\n", tins.len())
    };
    for t in tins {
        let kind = if t.kind == "channel" {
            "channel binary"
        } else {
            "source tin"
        };
        let latest = t.latest_version.as_deref().unwrap_or("unpublished");
        out.push_str(&format!("- {} ({kind}, latest {latest})", t.name));
        if let Some(desc) = t.description.as_deref().filter(|s| !s.is_empty()) {
            out.push_str(&format!(" — {desc}"));
        }
        if !t.tags.is_empty() {
            out.push_str(&format!(" [{}]", t.tags.join(", ")));
        }
        match t.verified_ok {
            Some(true) => out.push_str(" (smoke build passing)"),
            Some(false) => out.push_str(" (smoke build FAILING)"),
            None => {}
        }
        out.push('\n');
    }
    out.push_str("\nCall tin_info with a name for the full card.");
    out
}

/// The card minus its API-surface dump: title, naming bullets, install, and
/// usage — what an agent needs to actually use the tin.
fn usage_sections(card: &str) -> String {
    let mut out = String::new();
    let mut seen_section = false;
    let mut in_kept_section = false;
    for line in card.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            seen_section = true;
            in_kept_section = title == "Install" || title.starts_with("Usage");
        }
        let keep = if seen_section {
            in_kept_section
        } else {
            line.starts_with("# ")
                || line.starts_with("- package name")
                || line.starts_with("- Mojo import name")
        };
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !out.contains("## Usage") {
        out.push_str(
            "\n(No usage snippet has been extracted from this tin's README \
             yet — see the repository's README for examples.)\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(name: &str, kind: &str, verified: Option<bool>) -> TinSummary {
        TinSummary {
            nightly_at: None,
            nightly_ok: None,
            nightly_compiler: None,
            name: name.into(),
            url: format!("https://github.com/o/{name}.git"),
            description: Some(format!("{name} library")),
            author: Some("someone".into()),
            tags: vec!["parsing".into()],
            latest_version: Some("0.1.0".into()),
            kind: kind.into(),
            stars: None,
            forks: None,
            score: None,
            last_push: None,
            prev_url: None,
            url_changed_at: None,
            verified_at: verified.map(|_| "2026-08-27T00:00:00Z".into()),
            verified_ok: verified,
            verified_compiler: None,
        }
    }

    #[test]
    fn search_rendering_covers_kinds_and_emptiness() {
        let tins = vec![
            summary("csv", "source", Some(true)),
            summary("emberjson", "channel", None),
        ];
        let out = render_search(&tins, "parsing");
        assert!(out.contains("2 tin(s) matching \"parsing\""));
        assert!(out.contains(
            "- csv (source tin, latest 0.1.0) — csv library [parsing] (smoke build passing)"
        ));
        assert!(out.contains("- emberjson (channel binary, latest 0.1.0)"));
        assert!(out.contains("tin_info"));

        let none = render_search(&[], "quantum");
        assert!(none.contains("No tins match \"quantum\""));
        assert!(none.contains("llms.txt"));

        let all = render_search(&tins, "");
        assert!(all.starts_with("All 2 tins on the shelf:"));
    }

    #[test]
    fn usage_keeps_install_and_naming_drops_api_dump() {
        let card = "# zlib-mojo (source tin)\n\nzlib bindings\n\n\
            - package name (registry/conda): `zlib-mojo`\n\
            - Mojo import name: `zlib` (e.g. `from zlib import …`)\n\
            - repository: https://github.com/o/zlib.mojo.git\n\n\
            ## Install\n\n```sh\npixi shelf add zlib-mojo\n```\n\n\
            ## API surface\n\n### src/zlib/inflate.mojo\n\n- `fn inflate(...)`\n\n\
            ## Usage (from the README)\n\n```mojo\nfrom zlib import inflate\n```\n";
        let out = usage_sections(card);
        assert!(out.contains("# zlib-mojo"));
        assert!(out.contains("Mojo import name: `zlib`"));
        assert!(out.contains("pixi shelf add zlib-mojo"));
        assert!(out.contains("from zlib import inflate"));
        assert!(!out.contains("API surface"));
        assert!(!out.contains("fn inflate(...)"));
        assert!(!out.contains("repository:"));
    }

    #[test]
    fn usage_notes_missing_snippet() {
        let card = "# csv (source tin)\n\n- package name (registry/conda): `csv`\n\n\
            ## Install\n\n```sh\nshelf add csv\n```\n";
        let out = usage_sections(card);
        assert!(out.contains("No usage snippet"));
    }

    #[test]
    fn initialize_echoes_supported_version_only() {
        let req = serde_json::json!({ "params": { "protocolVersion": "2025-03-26" } });
        assert_eq!(initialize_result(&req)["protocolVersion"], "2025-03-26");
        let unknown = serde_json::json!({ "params": { "protocolVersion": "1999-01-01" } });
        assert_eq!(
            initialize_result(&unknown)["protocolVersion"],
            LATEST_PROTOCOL
        );
        let tools = tool_definitions();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["search_tins", "tin_info", "usage_example"]);
    }
}
