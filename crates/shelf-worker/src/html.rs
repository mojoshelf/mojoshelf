//! Server-rendered pages: Tins (public index), Authors (dashboard), admin.

use crate::db::{TinRow, VersionRow};
use shelf_core::TinSummary;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn page(title: &str, active: &str, body: &str) -> String {
    let item = |href: &str, label: &str| {
        let class = if label == active { " class=\"active\"" } else { "" };
        format!("<a href=\"{href}\"{class}>{label}</a>")
    };
    let nav = format!(
        "<aside><div class=\"brand\">🔥 Mojo Shelf</div><nav>{}{}{}{}</nav></aside>",
        item("/", "Tins"),
        item("/authors", "Authors"),
        item("/getting-started", "Getting started"),
        item("/install-modes", "Install modes"),
    );
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
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
  table {{ border-collapse: collapse; width: 100%; }}
  th, td {{ text-align: left; padding: .4rem .6rem; border-bottom: 1px solid var(--border); }}
  code {{ background: var(--code-bg); padding: .1rem .3rem; border-radius: 3px; }}
  pre {{ background: var(--code-bg); padding: .6rem .8rem; border-radius: 6px;
        overflow-x: auto; }}
  pre code {{ background: none; padding: 0; }}
  form.tin {{ margin: 1rem 0; padding: 1rem; border: 1px solid var(--border);
              border-radius: 6px; }}
  form.inline {{ display: inline; }}
  input, textarea {{ width: 100%; box-sizing: border-box; margin: .2rem 0 .6rem; padding: .3rem; }}
  button.danger {{ background: var(--danger-bg); border: 1px solid var(--danger-border);
                  color: var(--danger-fg); border-radius: 4px; cursor: pointer; }}
  .token {{ background: var(--note-bg); border: 1px solid var(--note-border); padding: 1rem;
           border-radius: 6px; word-break: break-all; }}
  .pick-one {{ font-size: .85rem; font-weight: 400; color: var(--muted); }}
  .install-label {{ margin: .8rem 0 .25rem; font-size: .85rem; color: var(--muted); }}
  .install-label + pre {{ margin-top: 0; }}
  footer {{ margin-top: 2rem; font-size: .8rem; color: var(--muted); }}
</style>
</head>
<body>
{nav}
<main>
{body}
<footer>mojoshelf — an experimental registry of reusable Mojo tins, installed as pixi source dependencies or git submodules.</footer>
</main>
</body>
</html>"#
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
        .map(|n| format!("<li><a href=\"/tins/{n}\"><code>{n}</code></a></li>", n = esc(n)))
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
            format!(
                "<tr><td><a href=\"/tins/{name}\"><code>{name}</code></a></td>\
                 <td>{}</td><td>{}</td>\
                 <td><a href=\"{}\">{}</a></td><td>{}{}</td></tr>",
                b.latest_version.as_deref().map(esc).unwrap_or_else(|| "—".into()),
                author_link(b.author.as_deref()),
                esc(&b.url),
                esc(&b.url),
                esc(b.description.as_deref().unwrap_or("")),
                if tags.is_empty() { String::new() } else { format!("<br>{tags}") },
                name = esc(&b.name),
            )
        })
        .collect();
    format!(
        "<table><tr><th>tin</th><th>latest</th><th>author</th>\
         <th>repository</th><th>description</th></tr>{rows}</table>"
    )
}

pub fn home(tins: &[TinSummary], q: &str) -> String {
    let result_line = if q.is_empty() {
        String::new()
    } else {
        format!(
            "<p>{} tin{} matching <strong>{}</strong> — <a href=\"/\">clear</a></p>",
            tins.len(),
            if tins.len() == 1 { "" } else { "s" },
            esc(q),
        )
    };
    let body = format!(
        r#"<h1>Mojo Shelf</h1>
<p>A registry of reusable Mojo tins, installed as pixi source dependencies or git submodules.
New here? See <a href="/getting-started">Getting started</a>.</p>
<form class="search" method="get" action="/">
<input type="search" name="q" value="{q}" placeholder="Search name, description, tags…">
<button>Search</button>
</form>
{result_line}
<h2>Tins</h2>
{table}"#,
        q = esc(q),
        table = tin_table(tins),
    );
    page("Mojo Shelf", "Tins", &body)
}

pub fn tin(d: &shelf_core::TinDetail) -> String {
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
    let install = match d.versions.first() {
        Some(latest) => format!(
            r#"<h2>Install <span class="pick-one">— pick one</span></h2>
<p class="install-label">the <code>pixi shelf</code> and <code>shelf</code> options need the
shelf extension installed once:
<code>pixi global install --channel https://mojoshelf.org/channel mojoshelf</code></p>
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
<p>{desc}</p>
<p>by {author} — <a href="{url}">{url}</a></p>
<p>{tags}</p>
{install}
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
<h2>Which should I use?</h2>
<p>Prefer <a href="/getting-started">pixi mode</a> when every tin you need
supports it; fall back to submodules otherwise. The two modes coexist in one
project. Submodule mode will be retired only when every published tin is
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
<code>-I</code> flags. Enable pixi's preview feature in your workspace:</p>
<pre><code>[workspace]
preview = ["pixi-build"]</code></pre>
<p>Then, from anywhere in the workspace:</p>
<pre><code>pixi shelf add &lt;name&gt;      # or: shelf add --pixi &lt;name&gt;</code></pre>
<p>The tin and its dependencies are added flat, each pinned to its published
commit via <code>pixi add --git … --rev …</code>. Note: pixi-build is a pixi
preview feature, and the tin must be a pixi package (a
<code>[package]</code> section with the pixi-build-mojo backend) — tins on
the shelf are still adopting this; submodule mode works for every tin
today.</p>
<h2>Submodule mode</h2>
<p>From your project's repo root:</p>
<pre><code>shelf add &lt;name&gt;</code></pre>
<p>The tin and its dependencies land as flat submodules under
<code>shelf/</code>, pinned to their published commits. Point the Mojo
compiler at them with <code>-I</code>, wrapped as a pixi task:</p>
<pre><code>[tasks]
run = "mojo run -I shelf/csv/src src/main.mojo"</code></pre>
<p>See <a href="https://github.com/mojoshelf/example">mojoshelf/example</a> for a
complete working project consuming the <a href="/tins/csv">csv</a> tin —
clone it with <code>--recurse-submodules</code> and <code>pixi run run</code>.</p>
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
        tins
            .iter()
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
