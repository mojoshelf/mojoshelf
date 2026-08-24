//! Server-rendered pages: Books (public index), Authors (dashboard), admin.

use crate::db::{BookRow, VersionRow};
use shelf_core::BookSummary;

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
        "<aside><div class=\"brand\">🔥 Mojo Shelf</div><nav>{}{}{}</nav></aside>",
        item("/", "Books"),
        item("/authors", "Authors"),
        item("/getting-started", "Getting started"),
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
  table {{ border-collapse: collapse; width: 100%; }}
  th, td {{ text-align: left; padding: .4rem .6rem; border-bottom: 1px solid var(--border); }}
  code {{ background: var(--code-bg); padding: .1rem .3rem; border-radius: 3px; }}
  pre {{ background: var(--code-bg); padding: .6rem .8rem; border-radius: 6px;
        overflow-x: auto; }}
  pre code {{ background: none; padding: 0; }}
  form.book {{ margin: 1rem 0; padding: 1rem; border: 1px solid var(--border);
              border-radius: 6px; }}
  form.inline {{ display: inline; }}
  input, textarea {{ width: 100%; box-sizing: border-box; margin: .2rem 0 .6rem; padding: .3rem; }}
  button.danger {{ background: var(--danger-bg); border: 1px solid var(--danger-border);
                  color: var(--danger-fg); border-radius: 4px; cursor: pointer; }}
  .token {{ background: var(--note-bg); border: 1px solid var(--note-border); padding: 1rem;
           border-radius: 6px; word-break: break-all; }}
  footer {{ margin-top: 2rem; font-size: .8rem; color: var(--muted); }}
</style>
</head>
<body>
{nav}
<main>
{body}
<footer>mojoshelf — an experimental, git-submodule-based registry of reusable Mojo books.</footer>
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

fn book_link_list(names: &[String]) -> String {
    if names.is_empty() {
        return "<p>None.</p>".into();
    }
    let items: String = names
        .iter()
        .map(|n| format!("<li><a href=\"/books/{n}\"><code>{n}</code></a></li>", n = esc(n)))
        .collect();
    format!("<ul>{items}</ul>")
}

fn book_table(books: &[BookSummary]) -> String {
    if books.is_empty() {
        return "<p>No books on the shelf yet.</p>".into();
    }
    let rows: String = books
        .iter()
        .map(|b| {
            let tags: String = b
                .tags
                .iter()
                .map(|t| format!("<span class=\"tag\">{}</span>", esc(t)))
                .collect();
            format!(
                "<tr><td><a href=\"/books/{name}\"><code>{name}</code></a></td>\
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
        "<table><tr><th>book</th><th>latest</th><th>author</th>\
         <th>repository</th><th>description</th></tr>{rows}</table>"
    )
}

pub fn home(books: &[BookSummary]) -> String {
    let body = format!(
        r#"<h1>Mojo Shelf</h1>
<p>A registry of reusable Mojo books, installed as git submodules.
New here? See <a href="/getting-started">Getting started</a>.</p>
<h2>Books</h2>
{}"#,
        book_table(books)
    );
    page("Mojo Shelf", "Books", &body)
}

pub fn book(d: &shelf_core::BookDetail) -> String {
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
                    .map(|n| format!("<a href=\"/books/{n}\"><code>{n}</code></a> ", n = esc(n)))
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
    let body = format!(
        r#"<h1><code>{name}</code></h1>
<p>{desc}</p>
<p>by {author} — <a href="{url}">{url}</a></p>
<p>{tags}</p>
<pre><code>shelf add {name}</code></pre>
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
        depends_on = book_link_list(&depends_on),
        dependents = book_link_list(&d.dependents),
    );
    page(&format!("Mojo Shelf — {}", d.name), "Books", &body)
}

pub fn author(login: &str, books: &[BookSummary]) -> String {
    let body = format!(
        "<h1>{login}</h1>\
         <p>Books published by <a href=\"https://github.com/{login}\">{login}</a>:</p>{}",
        book_table(books),
        login = esc(login),
    );
    page(&format!("Mojo Shelf — {login}"), "Books", &body)
}

pub fn getting_started() -> String {
    let body = r#"<h1>Getting started</h1>
<p>Install the <code>shelf</code> CLI:</p>
<pre><code>cargo install --locked --git https://github.com/mojoshelf/mojoshelf mojoshelf</code></pre>
<p>Then, from your project's repo root, add a book from the
<a href="/">shelf</a>:</p>
<pre><code>shelf add &lt;name&gt;</code></pre>
<p>The book and its dependencies land as submodules under <code>shelf/</code>,
pinned to their published commits. Useful commands:</p>
<table>
<tr><td><code>shelf search [term]</code></td><td>search the registry (name, description, tags)</td></tr>
<tr><td><code>shelf info &lt;name&gt;</code></td><td>show a book's versions and dependencies</td></tr>
<tr><td><code>shelf list</code></td><td>list installed books with pinned versions</td></tr>
<tr><td><code>shelf update [&lt;name&gt;]</code></td><td>re-pin to the latest published versions</td></tr>
<tr><td><code>shelf remove &lt;name&gt;</code></td><td>remove a book's submodule</td></tr>
</table>
<h2>Build with pixi</h2>
<p>Point the Mojo compiler at the installed books with <code>-I</code>, wrapped
as a pixi task:</p>
<pre><code>[tasks]
run = "mojo run -I shelf/csv/src src/main.mojo"</code></pre>
<p>See <a href="https://github.com/mojoshelf/example">mojoshelf/example</a> for a
complete working project consuming the <a href="/books/csv">csv</a> book —
clone it with <code>--recurse-submodules</code> and <code>pixi run run</code>.</p>
<p>Want to publish your own book? See the <a href="/authors">Authors</a> page.</p>"#;
    page("Mojo Shelf getting started", "Getting started", body)
}

fn publishing_section() -> &'static str {
    r#"<h2>Publishing a book</h2>
<p>A book is a public git repo with a <code>shelf.toml</code> at its root:</p>
<pre><code>name = "lightbug_http"
version = "0.2.0"
description = "HTTP framework for Mojo"
tags = ["http", "networking"]
# other books this one depends on; omit if none
books = ["small_time"]</code></pre>
<p><code>description</code>, <code>tags</code>, and <code>books</code> are
optional. Bump <code>version</code>, commit and push, then run
<code>shelf publish</code> from the repo root with your publish token
exported as <code>SHELF_TOKEN</code>; the registry takes the description and
tags from <code>shelf.toml</code> on every publish. Dependencies must already
be registered books.</p>"#
}

pub fn authors_signed_out() -> String {
    let body = format!(
        r#"<h1>Authors</h1>
<p>Sign in with GitHub to publish books, manage your publish token, and
delete versions or books you own.</p>
<p><a href="/auth/login"><button>Sign in with GitHub</button></a></p>
{}"#,
        publishing_section()
    );
    page("Mojo Shelf authors", "Authors", &body)
}

pub fn authors_dashboard(
    login: &str,
    has_token: bool,
    books: &[(BookRow, Vec<VersionRow>)],
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

    let books_section = if books.is_empty() {
        "<p>You have no books yet. Publish one with <code>shelf publish</code> \
         from your book's repo root.</p>"
            .to_string()
    } else {
        books
            .iter()
            .map(|(book, versions)| {
                let vrows: String = versions
                    .iter()
                    .map(|v| {
                        format!(
                            r#"<tr><td>{version}</td><td><code>{sha}</code></td><td>{date}</td>
<td><form class="inline" method="post" action="/authors/books/{name}/versions/{version}/delete"
 onsubmit="return confirm('Delete {name} {version}?')">
<button class="danger">delete</button></form></td></tr>"#,
                            name = esc(&book.name),
                            version = esc(&v.version),
                            sha = esc(&v.commit_sha[..12.min(v.commit_sha.len())]),
                            date = esc(&v.published_at),
                        )
                    })
                    .collect();
                format!(
                    r#"<h3><code>{name}</code></h3>
<p>{url}
<form class="inline" method="post" action="/authors/books/{name}/delete"
 onsubmit="return confirm('Delete the whole book {name} and all its versions?')">
<button class="danger">delete book</button></form></p>
<table><tr><th>version</th><th>commit</th><th>published</th><th></th></tr>{vrows}</table>"#,
                    name = esc(&book.name),
                    url = esc(&book.url),
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
<h2>Your books</h2>
{books_section}
{publishing}"#,
        login = esc(login),
        publishing = publishing_section(),
    );
    page("Mojo Shelf authors", "Authors", &body)
}

pub fn admin(books: &[BookSummary], email: &str) -> String {
    let forms: String = books
        .iter()
        .map(|b| {
            format!(
                r#"<form class="book" method="post" action="/admin/books">
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
<h2>Register a book</h2>
<form class="book" method="post" action="/admin/books">
<label>Name <input name="name" required pattern="[a-z0-9_-]+"></label>
<label>Git URL <input name="url" required></label>
<label>Description <input name="description"></label>
<button>Register</button>
</form>
<h2>Edit books</h2>
{forms}"#,
        email = esc(email),
    );
    page("Mojo Shelf admin", "Books", &body)
}
