//! Server-rendered pages: Tins (public index), Authors (dashboard), admin.

use crate::db::{TinRow, VersionRow};
use shelf_core::TinSummary;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Adds a copy button to every <pre> block. Kept as a plain constant so the
/// braces stay out of the format! template.
const COPY_SCRIPT: &str = r#"<script>
document.querySelectorAll("pre").forEach(function (pre) {
  var btn = document.createElement("button");
  btn.className = "copy-btn";
  btn.type = "button";
  btn.textContent = "copy";
  btn.addEventListener("click", function () {
    var code = pre.querySelector("code");
    navigator.clipboard.writeText((code || pre).textContent).then(function () {
      btn.textContent = "copied!";
      setTimeout(function () { btn.textContent = "copy"; }, 1500);
    });
  });
  pre.appendChild(btn);
});
</script>"#;

/// PostHog product analytics (US cloud). The project API key is a public
/// client-side token (safe to embed); until a real `phc_` key is set the
/// snippet is omitted entirely. Loaded deferred so it never blocks render.
pub(crate) const POSTHOG_KEY: &str = "phc_n3y95XLFqPzcgcJ34J7HKMCQrC7Ysi5WmGnvhwJbkaCy";

fn posthog_snippet() -> String {
    if !POSTHOG_KEY.starts_with("phc_") || POSTHOG_KEY == "phc_TODO" {
        return String::new();
    }
    format!(
        r#"<script defer src="/ph/static/array.js"></script>
<script>
window.addEventListener('DOMContentLoaded', function () {{
  if (window.posthog) posthog.init('{POSTHOG_KEY}', {{
    api_host: 'https://mojoshelf.org/ph',
    ui_host: 'https://us.posthog.com',
    defaults: '2025-05-24',
    persistence: 'memory',
    capture_exceptions: true
  }});
}});
</script>"#
    )
}

fn page(title: &str, active: &str, body: &str) -> String {
    let item = |href: &str, label: &str| {
        let class = if label == active {
            " class=\"active\""
        } else {
            ""
        };
        format!("<a href=\"{href}\"{class}>{label}</a>")
    };
    let nav = format!(
        "<aside><div class=\"brand\">🔥 Mojo Shelf</div><nav>{}{}{}{}{}{}</nav></aside>",
        item("/", "Tins"),
        item("/authors", "Authors"),
        item("/getting-started", "Getting started"),
        item("/install-modes", "Install modes"),
        item("/packaging", "Packaging"),
        item("/community-channel", "Community channel"),
    );
    let posthog = posthog_snippet();
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="description" content="mojoshelf — an experimental community registry of reusable Mojo libraries (tins), installed as pixi source dependencies or git submodules. Not affiliated with Modular.">
<meta property="og:site_name" content="Mojo Shelf">
<meta property="og:title" content="{title}">
<meta property="og:description" content="An experimental community registry of reusable Mojo libraries (tins).">
<title>{title}</title>
<style>
  :root {{
    color-scheme: light dark;
    --bg: #ffffff; --fg: #1a1a1a; --muted: #555; --border: #ddd;
    --code-bg: #f4f4f4;
    --accent: #f4900c; /* 🔥 orange */
    --danger-bg: #fee; --danger-border: #c66; --danger-fg: #900;
    --note-bg: #fff8e0; --note-border: #e0c860;
  }}
  @media (prefers-color-scheme: dark) {{
    :root {{
      --bg: #141414; --fg: #e6e6e6; --muted: #a5a5a5; --border: #383838;
      --code-bg: #262626;
      --accent: #ffa733;
      --danger-bg: #3a1616; --danger-border: #a05252; --danger-fg: #ff9d9d;
      --note-bg: #322a10; --note-border: #8a742e;
    }}
  }}
  body {{ font-family: ui-sans-serif, system-ui, sans-serif; max-width: 64rem;
         margin: 2rem auto; padding: 0 1rem; background: var(--bg); color: var(--fg);
         display: flex; gap: 2rem; align-items: flex-start; }}
  aside {{ flex: 0 0 11rem; position: sticky; top: 2rem; }}
  aside .brand {{ font-weight: 700; margin-bottom: 1rem; }}
  main {{ flex: 1; min-width: 0; }}
  h1 {{ font-size: 1.5rem; margin-top: 0; }}
  a {{ color: var(--accent); }}
  nav {{ display: flex; flex-direction: column; border-left: 1px solid var(--border); }}
  nav a {{ padding: .35rem .8rem; text-decoration: none; color: var(--muted);
          border-left: 2px solid transparent; margin-left: -1px; }}
  nav a.active {{ border-left-color: var(--accent); color: var(--fg); font-weight: 600; }}
  @media (max-width: 40rem) {{
    body {{ flex-direction: column; }}
    aside {{ position: static; flex: none; width: 100%; }}
    nav {{ flex-direction: row; border-left: none; border-bottom: 1px solid var(--border); }}
    nav a {{ border-left: none; border-bottom: 2px solid transparent; margin: 0; }}
    nav a.active {{ border-bottom-color: var(--accent); }}
  }}
  .tag {{ display: inline-block; background: var(--code-bg); color: var(--muted);
         border-radius: 10px; padding: 0 .5rem; font-size: .8rem; margin: 0 .15rem .15rem 0; }}
  form.search {{ display: flex; gap: .5rem; margin: 1rem 0; }}
  form.search input {{ flex: 1; margin: 0; padding: .45rem .6rem;
                      border: 1px solid var(--border); border-radius: 6px;
                      background: var(--bg); color: var(--fg); }}
  form.search button {{ padding: .45rem .9rem; border: 1px solid var(--accent);
                       background: var(--accent); color: #fff; border-radius: 6px;
                       cursor: pointer; }}
  table {{ border-collapse: collapse; width: 100%; table-layout: auto; }}
  /* Second line of the repository cell: the author who published the tin. */
  .sub {{ display: block; font-size: .8rem; color: var(--muted); font-weight: 400; }}
  /* Tin list: fixed layout, so these widths are honoured exactly. Under the
     default auto layout a percentage is only a hint and the browser hands
     leftover space to whichever column it likes — which was starving
     description, the one column that actually needs the room. */
  .tins {{ table-layout: fixed; }}
  .tins th:nth-child(1), .tins td:nth-child(1) {{ width: 13%; }}
  .tins th:nth-child(2), .tins td:nth-child(2) {{ width: 7%; }}
  .tins th:nth-child(3), .tins td:nth-child(3) {{ width: 20%; }}
  .tins th:nth-child(4), .tins td:nth-child(4) {{ width: 8%; white-space: nowrap; }}
  .tins th:nth-child(5), .tins td:nth-child(5) {{ width: 52%; }}
  /* Fixed columns cannot grow, so long names break instead of overflowing. */
  .tins td {{ overflow-wrap: anywhere; }}
  th, td {{ text-align: left; padding: .4rem .6rem; border-bottom: 1px solid var(--border); }}
  code {{ background: var(--code-bg); padding: .1rem .3rem; border-radius: 3px; }}
  pre {{ background: var(--code-bg); padding: .6rem .8rem; border-radius: 6px;
        overflow-x: auto; position: relative; }}
  pre code {{ background: none; padding: 0; }}
  .copy-btn {{ position: absolute; top: .4rem; right: .4rem; font-size: .75rem;
              padding: .1rem .5rem; border: 1px solid var(--border);
              border-radius: 4px; background: var(--bg); color: var(--muted);
              cursor: pointer; opacity: 0; transition: opacity .15s; }}
  pre:hover .copy-btn, .copy-btn:focus {{ opacity: 1; }}
  form.tin {{ margin: 1rem 0; padding: 1rem; border: 1px solid var(--border);
              border-radius: 6px; }}
  form.inline {{ display: inline; }}
  input, textarea {{ width: 100%; box-sizing: border-box; margin: .2rem 0 .6rem; padding: .3rem; }}
  button.danger {{ background: var(--danger-bg); border: 1px solid var(--danger-border);
                  color: var(--danger-fg); border-radius: 4px; cursor: pointer; }}
  .token {{ background: var(--note-bg); border: 1px solid var(--note-border); padding: 1rem;
           border-radius: 6px; word-break: break-all; }}
  .warn {{ background: var(--danger-bg); border: 1px solid var(--danger-border);
          color: var(--danger-fg); padding: .6rem .8rem; border-radius: 6px; }}
  .warn a {{ color: inherit; }}
  .warn-tag {{ display: inline-block; background: var(--danger-bg); color: var(--danger-fg);
              border: 1px solid var(--danger-border); border-radius: 10px;
              padding: 0 .5rem; font-size: .8rem; margin: 0 .15rem .15rem 0; }}
  .pick-one {{ font-size: .85rem; font-weight: 400; color: var(--muted); }}
  .install-label {{ margin: .8rem 0 .25rem; font-size: .85rem; color: var(--muted); }}
  .install-label + pre {{ margin-top: 0; }}
  .top-links {{ display: flex; justify-content: flex-end; gap: 1rem;
               font-size: .85rem; margin-bottom: .25rem; }}
  .top-links a {{ color: var(--muted); text-decoration: none; }}
  .top-links a:hover {{ color: var(--accent); }}
  /* The failing check's own error, quoted verbatim under the verdict. */
  .reason {{ margin: .2rem 0 .8rem; padding: .5rem .6rem; font-size: .8rem;
            background: var(--danger-bg); color: var(--danger-fg);
            border: 1px solid var(--danger-border); border-radius: 6px;
            white-space: pre-wrap; overflow-wrap: anywhere; }}
  .pager {{ display: flex; gap: 1rem; align-items: center; margin-top: 1rem;
           font-size: .9rem; color: var(--muted); }}
  footer {{ margin-top: 2rem; font-size: .8rem; color: var(--muted); }}
</style>
{posthog}
</head>
<body>
{nav}
<main>
<div class="top-links">
<a href="https://github.com/mojoshelf/mojoshelf/issues">Issues</a>
<a href="https://github.com/mojoshelf/mojoshelf/discussions">Discussions</a>
</div>
{body}
<footer>mojoshelf — an experimental registry of reusable Mojo tins, installed as pixi source dependencies or git submodules.<br>
Anonymous usage analytics via PostHog, proxied first-party: no cookies, no cross-site tracking, no data sold. API and MCP requests are counted with a hashed, truncated identifier.</footer>
</main>
{copy_script}
</body>
</html>"#,
        copy_script = COPY_SCRIPT,
    )
}

/// "https://github.com/owner/repo.git" -> "owner/repo" (else the host).
fn short_repo(url: &str) -> String {
    match url.split("github.com/").nth(1) {
        Some(rest) => {
            let mut parts = rest.trim_end_matches(".git").split('/');
            match (parts.next(), parts.next()) {
                (Some(o), Some(r)) if !o.is_empty() && !r.is_empty() => format!("{o}/{r}"),
                _ => url.trim_start_matches("https://").to_string(),
            }
        }
        None => url
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap_or(url)
            .to_string(),
    }
}

/// "3d", "2mo", "1y" since an ISO timestamp; empty when unparsable.
fn age(iso: &str) -> String {
    let parsed = worker::js_sys::Date::parse(iso);
    if parsed.is_nan() {
        return String::new();
    }
    let days = ((worker::js_sys::Date::now() - parsed) / 86_400_000.0).max(0.0) as i64;
    if days < 1 {
        "today".into()
    } else if days < 60 {
        format!("{days}d")
    } else if days < 365 {
        format!("{}mo", days / 30)
    } else {
        format!("{}y", days / 365)
    }
}

/// Compact "⭐ 12 · 3d" cell; em dash before any data arrives.
fn activity_cell(stars: Option<i64>, last_push: Option<&str>) -> String {
    match (stars, last_push) {
        (Some(s), Some(p)) => {
            let a = age(p);
            if a.is_empty() {
                format!("⭐ {s}")
            } else if a == "today" {
                format!("⭐ {s}<span class=\"sub\">today</span>")
            } else {
                format!("⭐ {s}<span class=\"sub\">{a} ago</span>")
            }
        }
        (Some(s), None) => format!("⭐ {s}"),
        _ => "—".into(),
    }
}

/// True while a tin's URL change is recent enough to warn about
/// (shelf_core::URL_CHANGE_WARN_DAYS).
fn url_recently_changed(changed_at: Option<&str>) -> bool {
    changed_at
        .map(|c| {
            let now = (worker::js_sys::Date::now() / 1000.0) as i64;
            shelf_core::url_change_is_recent(c, now)
        })
        .unwrap_or(false)
}

/// Repo-swap warning banner for a tin page; empty once the change is old.
fn url_change_warning(url: &str, prev_url: Option<&str>, changed_at: Option<&str>) -> String {
    if !url_recently_changed(changed_at) {
        return String::new();
    }
    let changed = changed_at.unwrap_or_default();
    let from = prev_url
        .map(|p| {
            format!(
                " (previously <a href=\"{}\">{}</a>)",
                esc(p),
                esc(&short_repo(p))
            )
        })
        .unwrap_or_default();
    format!(
        "<p class=\"warn\">⚠️ The git repository behind this tin changed on {} to \
         <a href=\"{}\">{}</a>{}. Review the new repository before installing or \
         updating.</p>",
        esc(changed.get(..10).unwrap_or(changed)),
        esc(url),
        esc(&short_repo(url)),
        from,
    )
}

fn author_link(author: Option<&str>) -> String {
    match author {
        Some(a) => format!("<a href=\"/authors/{}\">{}</a>", esc(a), esc(a)),
        None => "—".into(),
    }
}

fn tin_link_list(names: &[String]) -> String {
    if names.is_empty() {
        return "<p>None.</p>".into();
    }
    let items: String = names
        .iter()
        .map(|n| {
            format!(
                "<li><a href=\"/tins/{n}\"><code>{n}</code></a></li>",
                n = esc(n)
            )
        })
        .collect();
    format!("<ul>{items}</ul>")
}

fn tin_table(tins: &[TinSummary]) -> String {
    if tins.is_empty() {
        return "<p>No tins on the shelf yet.</p>".into();
    }
    let rows: String = tins
        .iter()
        .map(|b| {
            let tags: String = b
                .tags
                .iter()
                .map(|t| format!("<span class=\"tag\">{}</span>", esc(t)))
                .collect();
            let badge = if b.kind == "channel" {
                " <span class=\"tag\">channel</span>".to_string()
            } else {
                String::new()
            };
            let repo_flag = if url_recently_changed(b.url_changed_at.as_deref()) {
                " <span class=\"warn-tag\" title=\"the git repository behind this \
                 tin recently changed\">⚠️ repo changed</span>"
            } else {
                ""
            };
            let author = if b.kind == "channel" {
                match b.author.as_deref() {
                    Some(a) => format!("<a href=\"https://github.com/{}\">{}</a>", esc(a), esc(a)),
                    None => "modular-community".to_string(),
                }
            } else {
                author_link(b.author.as_deref())
            };
            format!(
                "<tr><td><a href=\"/tins/{name}\"><code>{name}</code></a>{badge}</td>\
                 <td>{}</td>\
                 <td><a href=\"{}\">{}</a>{repo_flag}<span class=\"sub\">{author}</span></td>\
                 <td>{activity}</td><td>{}{}</td></tr>",
                b.latest_version
                    .as_deref()
                    .map(esc)
                    .unwrap_or_else(|| "—".into()),
                esc(&b.url),
                esc(&short_repo(&b.url)),
                esc(b.description.as_deref().unwrap_or("")),
                if tags.is_empty() {
                    String::new()
                } else {
                    format!("<br>{tags}")
                },
                name = esc(&b.name),
                badge = badge,
                activity = activity_cell(b.stars, b.last_push.as_deref()),
            )
        })
        .collect();
    format!(
        "<table class=\"tins\"><tr><th>tin</th><th>latest</th>\
         <th>repository<span class=\"sub\">author</span></th>\
         <th>activity</th><th>description</th></tr>{rows}</table>"
    )
}

/// Previous/next links, omitted entirely when everything fits on one page.
fn pager(q: &str, page: i64, last: i64) -> String {
    if last <= 1 {
        return String::new();
    }
    let href = |p: i64| {
        if q.is_empty() {
            format!("/?page={p}")
        } else {
            // Percent-encode for the query string, then escape for the
            // attribute: a search for "a & b" has to survive both.
            let encoded = String::from(worker::js_sys::encode_uri_component(q));
            format!("/?q={}&amp;page={p}", esc(&encoded))
        }
    };
    let link = |p: i64, label: &str| format!("<a href=\"{}\">{label}</a>", href(p));
    let mut parts = Vec::new();
    if page > 1 {
        parts.push(link(page - 1, "← previous"));
    }
    parts.push(format!("page {page} of {last}"));
    if page < last {
        parts.push(link(page + 1, "next →"));
    }
    format!("<p class=\"pager\">{}</p>", parts.join(" · "))
}

pub fn home(tins: &[TinSummary], page_no: i64, q: &str, last: i64, total: i64) -> String {
    let result_line = if q.is_empty() {
        String::new()
    } else {
        format!(
            "<p>{} tin{} matching <strong>{}</strong> — <a href=\"/\">clear</a></p>",
            total,
            if total == 1 { "" } else { "s" },
            esc(q),
        )
    };
    let body = format!(
        r#"<h1>Mojo Shelf</h1>
<p>A registry of reusable Mojo tins, installed as pixi source dependencies or git submodules.
New here? See <a href="/getting-started">Getting started</a>.</p>
<form class="search" method="get" action="/">
<input type="search" name="q" value="{q}" placeholder="Search name, description, tags, org, author…">
<button>Search</button>
</form>
{result_line}
<h2>{heading}</h2>
<p class="install-label">{ranking}Includes the
<a href="/community-channel">modular-community channel</a>, mirrored here —
its packages are badged <span class="tag">channel</span>.</p>
{table}
{pager}"#,
        q = esc(q),
        // Scores arrive in cron batches, so until the first refresh lands the
        // list is still alphabetical — don't claim a ranking it doesn't have.
        heading = if q.is_empty()
            && page_no == 1
            && last > 1
            && tins.first().is_some_and(|t| t.score.is_some())
        {
            format!("Most interesting tins <span class=\"pick-one\">of {total}</span>")
        } else {
            "Tins".to_string()
        },
        ranking = if tins.first().is_some_and(|t| t.score.is_some()) {
            "Ranked by stars, forks and recent commit activity.\n"
        } else {
            ""
        },
        table = tin_table(tins),
        pager = pager(q, page_no, last),
    );
    page("Mojo Shelf", "Tins", &body)
}

fn liveliness_line(d: &shelf_core::TinDetail) -> String {
    let (Some(stars), Some(push)) = (d.stars, d.last_push.as_deref()) else {
        return String::new();
    };
    let mut parts = vec![
        format!("⭐ {stars}"),
        format!("last commit {} ago", age(push)),
    ];
    if let Some(m) = d.commits_month {
        parts.push(format!(
            "{m} commit{} last month",
            if m == 1 { "" } else { "s" }
        ));
    }
    if let Some(y) = d.commits_year {
        parts.push(format!("{y} last year"));
    }
    format!("<p class=\"install-label\">{}</p>", parts.join(" · "))
}

/// Every tin-smoke run. The workflow cannot be filtered per tin from a URL, so
/// this is the fallback when a verification predates run-url recording.
const SMOKE_HISTORY: &str =
    "https://github.com/mojoshelf/mojoshelf/actions/workflows/tin-smoke.yml";

/// The error a failing check hit, shown inline. A link to a run page still
/// leaves the reader to open a job, expand a step and scroll; the one line
/// that explains the failure belongs on the page itself.
fn failure_reason(reason: Option<&str>) -> String {
    match reason {
        Some(r) if !r.trim().is_empty() => {
            format!("<pre class=\"reason\"><code>{}</code></pre>", esc(r.trim()))
        }
        _ => String::new(),
    }
}

/// Link to the run behind a verdict, or to the workflow's history when the
/// verification predates run-url recording.
fn run_link(url: Option<&str>) -> String {
    match url {
        Some(url) => format!(" — <a href=\"{}\">run log</a>", esc(url)),
        None => format!(" — <a href=\"{SMOKE_HISTORY}\">history</a>"),
    }
}

/// "✓ consumer smoke build passed…" line from the weekly tin-smoke run.
fn verification_line(d: &shelf_core::TinDetail) -> String {
    let when = |at: &str| {
        let a = age(at);
        if a.is_empty() || a == "today" {
            "today".to_string()
        } else {
            format!("{a} ago")
        }
    };
    // "failing" is only actionable if the logs are one click away, so link the
    // run behind the verdict — and the workflow's history when there is no run
    // recorded, which is every verification from before it was stored.
    match (d.verified_ok, d.verified_at.as_deref()) {
        (Some(true), Some(at)) => {
            let compiler = d
                .verified_compiler
                .as_deref()
                .map(|c| format!(" with mojo-compiler {}", esc(c)))
                .unwrap_or_default();
            format!(
                "<p class=\"install-label\">✓ consumer smoke build passed{compiler} — checked {}{}</p>",
                when(at),
                run_link(d.verified_run_url.as_deref()),
            )
        }
        (Some(false), Some(at)) => format!(
            "<p class=\"install-label\">✗ consumer smoke build failing (checked {}){}</p>{}",
            when(at),
            run_link(d.verified_run_url.as_deref()),
            failure_reason(d.verified_reason.as_deref()),
        ),
        _ => String::new(),
    }
}

/// "mojo nightly: passing…" line — the early-warning signal, shown only
/// once a nightly check has run.
fn nightly_line(d: &shelf_core::TinDetail) -> String {
    match (d.nightly_ok, d.nightly_at.as_deref()) {
        (Some(ok), Some(at)) => {
            let a = age(at);
            let when = if a.is_empty() || a == "today" {
                "today".to_string()
            } else {
                format!("{a} ago")
            };
            let compiler = d
                .nightly_compiler
                .as_deref()
                .map(|c| format!(" with mojo-compiler {}", esc(c)))
                .unwrap_or_default();
            let link = run_link(d.nightly_run_url.as_deref());
            if ok {
                format!(
                    "<p class=\"install-label\">✓ mojo nightly build passing{compiler} — checked {when}{link}</p>"
                )
            } else {
                format!(
                    "<p class=\"install-label\">✗ mojo nightly build failing (checked {when}){link}</p>{}",
                    failure_reason(d.nightly_reason.as_deref()),
                )
            }
        }
        _ => String::new(),
    }
}

/// Copyable README badge markdown for source tins.
fn badge_section(d: &shelf_core::TinDetail) -> String {
    if d.kind == "channel" {
        return String::new();
    }
    let name = esc(&d.name);
    format!(
        r#"<h2>Badge</h2>
<p class="install-label">for this tin's README — stable verification, and optionally the nightly signal</p>
<pre><code>[![mojoshelf](https://mojoshelf.org/badge/{name}.svg)](https://mojoshelf.org/tins/{name})
[![mojo nightly](https://mojoshelf.org/badge/{name}/nightly.svg)](https://mojoshelf.org/tins/{name})</code></pre>"#
    )
}

pub fn tin(d: &shelf_core::TinDetail) -> String {
    if d.kind == "channel" {
        let maintainer = match d.author.as_deref() {
            Some(a) => format!(
                " Maintained by <a href=\"https://github.com/{}\">{}</a>.",
                esc(a),
                esc(a)
            ),
            None => String::new(),
        };
        let desc = d
            .description
            .as_deref()
            .map(|s| format!("<p>{}</p>", esc(s)))
            .unwrap_or_default();
        let body = format!(
            r#"<h1><code>{name}</code> <span class="tag">channel</span></h1>
{warning}
{desc}
<p>A binary package from the
<a href="/community-channel">modular-community channel</a> — latest version
<strong>{version}</strong>.{maintainer}
Repository / details: <a href="{url}">{short}</a>.</p>
{liveliness}
<h2>Install <span class="pick-one">— pick one</span></h2>
<p class="install-label">with the shelf extension</p>
<pre><code>pixi shelf add {name}</code></pre>
<p class="install-label">or plain pixi (channel in your channel list)</p>
<pre><code>pixi add {name}</code></pre>
<p>Dependencies are resolved by conda — no registry pinning; your
<code>pixi.lock</code> records the solved version. Note: channel packages
carry their own <code>mojo-compiler</code> requirement, which may conflict
with a workspace pinned to a different Mojo era — the solver reports it if
so.</p>"#,
            name = esc(&d.name),
            version = esc(d.channel_version.as_deref().unwrap_or("?")),
            url = esc(&d.url),
            short = esc(&short_repo(&d.url)),
            desc = desc,
            maintainer = maintainer,
            liveliness = liveliness_line(d),
            warning =
                url_change_warning(&d.url, d.prev_url.as_deref(), d.url_changed_at.as_deref()),
        );
        return page(&format!("Mojo Shelf — {}", d.name), "Tins", &body);
    }
    let tags: String = d
        .tags
        .iter()
        .map(|t| format!("<span class=\"tag\">{}</span>", esc(t)))
        .collect();
    let vrows: String = d
        .versions
        .iter()
        .map(|v| {
            let deps: String = if v.dependencies.is_empty() {
                "—".into()
            } else {
                v.dependencies
                    .iter()
                    .map(|n| format!("<a href=\"/tins/{n}\"><code>{n}</code></a> ", n = esc(n)))
                    .collect()
            };
            format!(
                "<tr><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td></tr>",
                esc(&v.version),
                esc(&v.commit_sha[..12.min(v.commit_sha.len())]),
                esc(&v.published_at),
                deps,
            )
        })
        .collect();
    let depends_on = d
        .versions
        .first()
        .map(|v| v.dependencies.clone())
        .unwrap_or_default();
    let liveliness = liveliness_line(d);
    let graduated = match &d.channel_version {
        Some(cv) if d.kind == "source" => {
            let latest_src = d
                .versions
                .first()
                .map(|v| v.version.as_str())
                .unwrap_or("0.0.0");
            let drift = match (
                semver::Version::parse(cv),
                semver::Version::parse(latest_src),
            ) {
                (Ok(c), Ok(s)) if c > s => format!(
                    " — <strong>newer than the shelf's {}</strong>",
                    esc(latest_src)
                ),
                _ => String::new(),
            };
            format!(
                r#"<p class="install-label">Graduated: also on the
<a href="/community-channel">modular-community channel</a> as
<strong>{}</strong>{} (<code>pixi add {}</code> for the binary).</p>"#,
                esc(cv),
                drift,
                esc(&d.name),
            )
        }
        _ => String::new(),
    };
    let install = match d.versions.first() {
        Some(latest) => format!(
            r#"<h2>Install <span class="pick-one">— pick one</span></h2>
<p class="install-label">the <code>pixi shelf</code> and <code>shelf</code> options need the
shelf extension installed once:</p>
<pre><code>pixi global install --channel https://mojoshelf.org/channel mojoshelf</code></pre>
<p class="install-label">pixi mode: a registry-pinned git source dependency</p>
<pre><code>pixi shelf add {name}</code></pre>
<p class="install-label">the same, with plain pixi (no shelf extension needed)</p>
<pre><code>pixi add --git {url} \
    --rev {sha} {name}</code></pre>
<p class="install-label">submodule mode: pinned source under <code>shelf/{name}</code></p>
<pre><code>shelf add {name}</code></pre>"#,
            name = esc(&d.name),
            url = esc(&d.url),
            sha = esc(&latest.commit_sha),
        ),
        None => "<p>No published versions yet.</p>".to_string(),
    };
    let body = format!(
        r#"<h1><code>{name}</code></h1>
{warning}
<p>{desc}</p>
<p>by {author} — <a href="{url}">{url}</a></p>
<p>{tags}</p>
{liveliness}
{verification}
{nightly}
{graduated}
{install}
{badge_md}
<h2>Depends on</h2>
{depends_on}
<h2>Depended on by</h2>
{dependents}
<h2>Versions</h2>
<table><tr><th>version</th><th>commit</th><th>published</th><th>dependencies</th></tr>
{vrows}</table>"#,
        name = esc(&d.name),
        desc = esc(d.description.as_deref().unwrap_or("")),
        author = author_link(d.author.as_deref()),
        url = esc(&d.url),
        depends_on = tin_link_list(&depends_on),
        dependents = tin_link_list(&d.dependents),
        warning = url_change_warning(&d.url, d.prev_url.as_deref(), d.url_changed_at.as_deref()),
        verification = verification_line(d),
        nightly = nightly_line(d),
        badge_md = badge_section(d),
    );
    page(&format!("Mojo Shelf — {}", d.name), "Tins", &body)
}

pub fn author(login: &str, tins: &[TinSummary]) -> String {
    let body = format!(
        "<h1>{login}</h1>\
         <p>Tins published by <a href=\"https://github.com/{login}\">{login}</a>:</p>{}",
        tin_table(tins),
        login = esc(login),
    );
    page(&format!("Mojo Shelf — {login}"), "Tins", &body)
}

pub fn install_modes() -> String {
    let body = r#"<h1>Install modes: pixi vs. submodules</h1>
<p>Tins install either as <strong>pixi git source dependencies</strong> or as
<strong>git submodules</strong>. Both are pinned by the registry to published
commits; they differ in who does the building and where the source lives.</p>
<h2>What pixi mode does better</h2>
<ul>
<li>No submodule ceremony — no <code>--recurse-submodules</code>, detached
HEADs, or <code>.gitmodules</code> noise in your repo.</li>
<li>No <code>-I</code> flag bookkeeping — pixi builds each tin into a
<code>.mojopkg</code> via the <code>pixi-build-mojo</code> backend and imports
just work.</li>
<li>FFI tins can ship their native shims as real conda artifacts instead of
"run this script first" steps.</li>
<li>It points where Modular's own tooling is heading, so migrating off
mojoshelf later means editing a dependency list, not unwinding vendored
submodules.</li>
</ul>
<h2>Why submodules still earn their keep</h2>
<ul>
<li><strong>Coverage</strong> — every tin supports submodule mode today;
pixi mode requires the tin to be a pixi package, which the shelf is still
adopting tin by tin.</li>
<li><strong>Stability</strong> — pixi-build is a preview feature and its
manifest schema can still change; submodules are boring, stable git.</li>
<li><strong>Source in your tree</strong> — the dependency's code sits in your
editor: greppable, LSP-navigable, patchable while debugging. With pixi
dependencies you get a built package in a conda environment instead. This
trade-off is permanent, not transitional.</li>
</ul>
<p><strong>Note — submodule mode means you do the build.</strong> Pixi mode
compiles tins for you; with submodules you point the Mojo compiler at each
tin's source yourself (<code>-I shelf/&lt;name&gt;/src</code>), and FFI tins
additionally need their native shim built and reachable at runtime. The build
is not a secret: every tin repo carries its own definition — the
<code>[package]</code> sections in its <code>pixi.toml</code> and, for shims,
a <code>shim/</code> or <code>ffi/</code> subpackage (CMake or a
rattler-build recipe). The shortcut: run <code>pixi install</code> inside the
tin's checkout and it builds everything, including the shim, into that
environment.</p>
<h2>Which should I use?</h2>
<p>Prefer <a href="/getting-started">pixi mode</a> when every tin you need
supports it; fall back to submodules otherwise. The two modes coexist in one
project. A stable tin can also be <em>graduated</em> — <code>shelf
graduate</code> turns its pixi-build setup into a binary conda package
submission for the
<a href="/community-channel">modular-community channel</a>. Submodule mode will be retired only when every published tin is
pixi-consumable and pixi-build has stabilized — and with a documented
migration path (<code>shelf remove</code> each tin, then
<code>pixi shelf add</code>).</p>
<h2>The naming pattern: one tin, two namespaces</h2>
<p>In pixi mode a tin's name becomes a <strong>conda package name</strong> in
the consumer's environment. A tin named after an existing conda package the
environment also needs (directly or transitively) makes the dependency
solver's job impossible — there cannot be two packages called
<code>zlib</code> in one environment, and the real one is required by half
the ecosystem.</p>
<p>The pattern:</p>
<ul>
<li><strong>Tin name</strong> — must be unique in the conda namespace.
Before publishing, check <code>pixi search &lt;name&gt;</code> against
conda-forge and the Modular channels. For bindings to an existing library,
the convention is an <code>-mojo</code> suffix: <code>zlib-mojo</code>,
<code>lancedb-mojo</code>.</li>
<li><strong>Mojo import name</strong> — independent of the tin name, set by
the backend's <code>[package.build.config.pkg]</code> <code>name</code>
field, so imports stay natural: <code>from zlib import inflate</code>,
<code>from lancedb import …</code>.</li>
<li><strong>Native shims</strong> — FFI tins ship their C or Rust shims as
pixi subpackages (<code>pixi-build-cmake</code>, or a
<code>pixi-build-rattler-build</code> recipe) that the tin run-depends on.
The shim builds during install and lands in the environment's
<code>lib/</code>, where the Mojo code dlopens it — consumers never run a
build script.</li>
</ul>"#;
    page("Mojo Shelf install modes", "Install modes", body)
}

pub fn packaging() -> String {
    let body = r#"<h1>Packaging: conda vs. wheel</h1>
<p>In pixi mode a tin is a <strong>source dependency</strong>: pixi fetches the
tin's repo at its registry-pinned commit and builds it into a conda package in
your environment. The conda machinery underneath does real work for us:</p>
<h2>What conda does for tins</h2>
<ul>
<li><strong>Builds from source, automatically.</strong> The
<code>pixi-build-mojo</code> backend compiles the tin's Mojo package
(<code>.mojopkg</code>) on install; C and Rust FFI shims build as sibling
packages (cmake or rattler-build recipes) the tin run-depends on.</li>
<li><strong>Import resolution for free.</strong> Packages land in
<code>$CONDA_PREFIX/lib/mojo</code>, where the Mojo compiler finds them — no
<code>-I</code> flags in your build commands.</li>
<li><strong>A native dependency graph.</strong> Shims link real libraries
(libz, OpenSSL, …) from conda-forge; conda resolves and installs them into the
same environment and relocates rpaths so everything loads at runtime.</li>
<li><strong>Toolchain coherence.</strong> Compiled Mojo packages are tied to a
compiler version; the solver enforces one <code>mojo-compiler</code> across
every tin in the environment.</li>
<li><strong>Transitive builds.</strong> A tin that imports another tin
(<code>docx</code> → <code>zlib-mojo</code>) declares it as a git-pinned host
dependency; the dependency is built into the build environment first.</li>
</ul>
<p>The same machinery distributes the <code>shelf</code> CLI itself: a conda
package on the static channel at <code>/channel</code>, installed with
<code>pixi global install</code>.</p>
<h2>A pypi/wheel backend?</h2>
<p>Wheels are a plausible <em>additional</em> distribution for pure-Mojo,
Python-facing tins: PyPI hosting is universal, <code>uv</code> is fast, plain
venvs could consume tins, and pixi speaks PyPI natively. What's missing today:
a PEP 517 build backend for Mojo (the wheel-world equivalent of
pixi-build-mojo), a convention for finding <code>.mojopkg</code>s in
<code>site-packages</code>, and per-wheel vendoring of native shim libraries —
wheels have no shared native dependency graph, so each shim must bundle its
dylibs. Conda stays the primary substrate; a wheel experiment would start with
a Python-interop tin like <code>pontoneer</code>.</p>"#;
    page("Mojo Shelf packaging", "Packaging", body)
}

pub fn community_channel() -> String {
    let body = r#"<h1>The modular-community channel</h1>
<p><a href="https://repo.prefix.dev/modular-community">modular-community</a>
is Modular's curated channel of community packages: maintainers submit
rattler-build recipes by pull request, and CI publishes <em>binary</em> conda
packages for multiple platforms.</p>
<h2>Mirrored on the shelf</h2>
<p>Every channel package appears here automatically — in the
<a href="/">tin list</a> and search, badged <span class="tag">channel</span>,
refreshed every six hours from the channel's repodata and enriched with the
maintainer, summary, and repository from its recipe. Installing is uniform:</p>
<pre><code>pixi shelf add emberjson    # or plain: pixi add emberjson</code></pre>
<p>Channel tins are plain conda dependencies: no registry pinning (your
<code>pixi.lock</code> records the solved version), and the conda solver
owns their dependency graphs. One caveat inherited from the ecosystem:
each package carries its own <code>mojo-compiler</code> requirement, so a
workspace pinned to a different Mojo era gets a clear solver conflict
rather than an install.</p>
<h2>How the two kinds differ</h2>
<p><strong>Source tins</strong> are git repos, registry-pinned to published
commits, built from source on install, published here with
<code>shelf publish</code>. <strong>Channel tins</strong> are prebuilt
binaries, curated upstream, mirrored read-only — they cannot be published
to, installed as submodules, or depended on by name pins. The two share one
conda namespace, so channel names are reserved: a publish under a channel
package's name is rejected.</p>
<h2>The graduation path</h2>
<p>When a source tin stabilizes, graduate it to the channel — the CLI does
the ceremony:</p>
<pre><code>shelf graduate    # from the tin's repo root</code></pre>
<p>It preflights the tin (pixi package layout, license, pushed commit,
summary), generates a channel-ready <code>recipe.yaml</code> — source pinned
to your commit, compiler range derived from your pin, a smoke test, license
and maintainer filled in — and prints the fork-and-PR steps. Iterate
source-first on the shelf; graduate for curated binary distribution.
mojoshelf is the fast-iteration layer that feeds the official ecosystem —
and retires the day official packaging makes it redundant.</p>"#;
    page("Mojo Shelf community channel", "Community channel", body)
}

pub fn getting_started() -> String {
    let body = r#"<h1>Getting started</h1>
<p>Install the CLI — this provides both <code>shelf</code> and the
<code>pixi shelf</code> extension:</p>
<pre><code>pixi global install --channel https://mojoshelf.org/channel mojoshelf</code></pre>
<p>(No pixi? <code>cargo install --locked --git
https://github.com/mojoshelf/mojoshelf mojoshelf</code> works too. The conda
package is currently built for osx-arm64.)</p>
<p>Tins install in one of two modes.</p>
<h2>Pixi mode: git source dependencies</h2>
<p>Tins become registry-pinned git dependencies in your <code>pixi.toml</code>,
built by the <code>pixi-build-mojo</code> backend — no submodules, no
<code>-I</code> flags. Enable pixi's preview feature in your workspace, with
channels for the Mojo toolchain and the
<a href="/community-channel">modular-community</a> packages tins may depend
on:</p>
<pre><code>[workspace]
preview = ["pixi-build"]
channels = [
    "conda-forge",
    "https://conda.modular.com/max-nightly",
    "https://repo.prefix.dev/modular-community",
]</code></pre>
<p>Then, from anywhere in the workspace:</p>
<pre><code>pixi shelf add &lt;name&gt;      # or: shelf add --pixi &lt;name&gt;</code></pre>
<p>The tin and its dependencies are added flat, each pinned to its published
commit via <code>pixi add --git … --rev …</code>. Note: pixi-build is a pixi
preview feature, and the tin must be a pixi package (a
<code>[package]</code> section with the pixi-build-mojo backend) — nearly
every tin on the shelf supports it.</p>
<p>See <a href="https://github.com/mojoshelf/example">mojoshelf/example</a> for a
complete working project consuming the <a href="/tins/csv">csv</a> tin this
way — clone it and <code>pixi run run</code>.</p>
<p>The shelf also mirrors the
<a href="/community-channel">modular-community channel</a>: its binary
packages appear in the tin list badged <span class="tag">channel</span>, and
<code>pixi shelf add</code> installs them the same way.</p>
<h2>Submodule mode</h2>
<p>From your project's repo root:</p>
<pre><code>shelf add &lt;name&gt;</code></pre>
<p>The tin and its dependencies land as flat submodules under
<code>shelf/</code>, pinned to their published commits. Point the Mojo
compiler at them with <code>-I</code>, wrapped as a pixi task:</p>
<pre><code>[tasks]
run = "mojo run -I shelf/csv/src src/main.mojo"</code></pre>
<h2>Useful commands</h2>
<table>
<tr><td><code>shelf search [term]</code></td><td>search the registry (name, description, tags)</td></tr>
<tr><td><code>shelf info &lt;name&gt;</code></td><td>show a tin's versions and dependencies</td></tr>
<tr><td><code>shelf list</code></td><td>list installed tins with pinned versions</td></tr>
<tr><td><code>shelf update [&lt;name&gt;]</code></td><td>re-pin to the latest published versions</td></tr>
<tr><td><code>shelf remove &lt;name&gt;</code></td><td>remove an installed tin</td></tr>
</table>
<p>Every command works in both modes; prefix with <code>pixi</code> (or pass
<code>--pixi</code>) for pixi mode.</p>
<h2>Agent skills</h2>
<p>Working with a coding agent? Two
<a href="https://agentskills.io">Agent Skills</a> ship with the
<a href="https://github.com/mojoshelf/mojoshelf">mojoshelf repo</a> — one
teaches your agent to consume tins, the other to publish them:</p>
<pre><code>npx skills add mojoshelf/mojoshelf                                  # pick interactively
npx skills add mojoshelf/mojoshelf --skill mojoshelf-consume --yes  # or one directly
npx skills add mojoshelf/mojoshelf --skill mojoshelf-publish --yes</code></pre>
<h2>Connect your agent (MCP)</h2>
<p>The registry runs a
<a href="https://modelcontextprotocol.io">Model Context Protocol</a> server at
<code>https://mojoshelf.org/mcp</code> — anonymous, read-only, no API key.
Connect it once and your agent gets three tools in every session:
<code>search_tins</code> (find libraries by topic),
<code>tin_info</code> (a tin's full card: Mojo import name, install commands,
API surface, build health), and <code>usage_example</code> (install commands
plus a copy-pasteable snippet). With Claude Code:</p>
<pre><code>claude mcp add --transport http mojoshelf https://mojoshelf.org/mcp</code></pre>
<p>Any other MCP-capable host (claude.ai, Cursor, VS Code, …) connects with
the same URL. Agents without MCP can fetch
<a href="/llms.txt">/llms.txt</a> (index) or
<a href="/llms-full.txt">/llms-full.txt</a> (every tin's card), or use the
JSON API (<code>/api/tins?q=…</code>).</p>
<p>Want to publish your own tin? See the <a href="/authors">Authors</a> page.</p>"#;
    page("Mojo Shelf getting started", "Getting started", body)
}

fn publishing_section() -> &'static str {
    r#"<h2>Publishing a tin</h2>
<p>A tin is a public git repo with a <code>shelf.toml</code> at its root:</p>
<pre><code>name = "lightbug_http"
version = "0.2.0"
description = "HTTP framework for Mojo"
tags = ["http", "networking"]
# other tins this one depends on; omit if none
tins = ["small_time"]</code></pre>
<p><code>description</code>, <code>tags</code>, and <code>tins</code> are
optional. Bump <code>version</code>, commit and push, then run
<code>shelf publish</code> from the repo root with your publish token
exported as <code>SHELF_TOKEN</code>; the registry takes the description and
tags from <code>shelf.toml</code> on every publish. Dependencies must already
be registered tins.</p>
<p><strong>Naming:</strong> the tin name doubles as a conda package name in
pixi mode, so pick one that no conda package already uses
(<code>pixi search &lt;name&gt;</code>) — bindings conventionally take an
<code>-mojo</code> suffix, while the Mojo import name stays natural. See
<a href="/install-modes">install modes</a>.</p>"#
}

pub fn authors_signed_out() -> String {
    let body = format!(
        r#"<h1>Authors</h1>
<p>Sign in with GitHub to publish tins, manage your publish token, and
delete versions or tins you own.</p>
<p><a href="/auth/login"><button>Sign in with GitHub</button></a></p>
{}"#,
        publishing_section()
    );
    page("Mojo Shelf authors", "Authors", &body)
}

pub fn authors_dashboard(
    login: &str,
    has_token: bool,
    tins: &[(TinRow, Vec<VersionRow>)],
    fresh_token: Option<&str>,
) -> String {
    let token_section = match fresh_token {
        Some(token) => format!(
            r#"<div class="token"><p><strong>Your publish token</strong> — copy it now;
it is shown only once:</p><p><code>{}</code></p>
<p>Use it as: <code>export SHELF_TOKEN={}</code></p></div>"#,
            esc(token),
            esc(token),
        ),
        None => {
            let label = if has_token {
                "Rotate publish token (invalidates the current one)"
            } else {
                "Generate publish token"
            };
            format!(
                r#"<form method="post" action="/authors/token"><button>{label}</button></form>"#
            )
        }
    };

    let tins_section = if tins.is_empty() {
        "<p>You have no tins yet. Publish one with <code>shelf publish</code> \
         from your tin's repo root.</p>"
            .to_string()
    } else {
        tins.iter()
            .map(|(tin, versions)| {
                let vrows: String = versions
                    .iter()
                    .map(|v| {
                        format!(
                            r#"<tr><td>{version}</td><td><code>{sha}</code></td><td>{date}</td>
<td><form class="inline" method="post" action="/authors/tins/{name}/versions/{version}/delete"
 onsubmit="return confirm('Delete {name} {version}?')">
<button class="danger">delete</button></form></td></tr>"#,
                            name = esc(&tin.name),
                            version = esc(&v.version),
                            sha = esc(&v.commit_sha[..12.min(v.commit_sha.len())]),
                            date = esc(&v.published_at),
                        )
                    })
                    .collect();
                format!(
                    r#"<h3><code>{name}</code></h3>
<p>{url}
<form class="inline" method="post" action="/authors/tins/{name}/delete"
 onsubmit="return confirm('Delete the whole tin {name} and all its versions?')">
<button class="danger">delete tin</button></form></p>
<table><tr><th>version</th><th>commit</th><th>published</th><th></th></tr>{vrows}</table>"#,
                    name = esc(&tin.name),
                    url = esc(&tin.url),
                )
            })
            .collect()
    };

    let body = format!(
        r#"<h1>Authors</h1>
<p>Signed in as <strong>{login}</strong>.
<form class="inline" method="post" action="/auth/logout"><button>Sign out</button></form></p>
<h2>Publish token</h2>
{token_section}
<h2>Your tins</h2>
{tins_section}
{publishing}"#,
        login = esc(login),
        publishing = publishing_section(),
    );
    page("Mojo Shelf authors", "Authors", &body)
}

pub fn admin(tins: &[TinSummary], email: &str) -> String {
    let forms: String = tins
        .iter()
        .map(|b| {
            format!(
                r#"<form class="tin" method="post" action="/admin/tins">
<strong>{name}</strong>
<input type="hidden" name="name" value="{name}">
<label>URL <input name="url" value="{url}" required></label>
<label>Description <input name="description" value="{desc}"></label>
<button>Save</button>
</form>"#,
                name = esc(&b.name),
                url = esc(&b.url),
                desc = esc(b.description.as_deref().unwrap_or("")),
            )
        })
        .collect();
    let body = format!(
        r#"<h1>Mojo Shelf admin</h1>
<p>Signed in as {email}.</p>
<h2>Register a tin</h2>
<form class="tin" method="post" action="/admin/tins">
<label>Name <input name="name" required pattern="[a-z0-9_-]+"></label>
<label>Git URL <input name="url" required></label>
<label>Description <input name="description"></label>
<button>Register</button>
</form>
<h2>Edit tins</h2>
{forms}"#,
        email = esc(email),
    );
    page("Mojo Shelf admin", "Tins", &body)
}
