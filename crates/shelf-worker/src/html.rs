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
    let tab = |href: &str, label: &str| {
        let class = if label == active { " class=\"active\"" } else { "" };
        format!("<a href=\"{href}\"{class}>{label}</a>")
    };
    let nav = format!("<nav>{} {}</nav>", tab("/", "Books"), tab("/authors", "Authors"));
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
  body {{ font-family: ui-sans-serif, system-ui, sans-serif; max-width: 52rem;
         margin: 2rem auto; padding: 0 1rem; background: var(--bg); color: var(--fg); }}
  h1 {{ font-size: 1.5rem; }}
  a {{ color: var(--accent); }}
  nav {{ margin-bottom: 1.5rem; border-bottom: 1px solid var(--border); }}
  nav a {{ display: inline-block; padding: .4rem .8rem; text-decoration: none;
          color: var(--muted); }}
  nav a.active {{ border-bottom: 2px solid var(--accent); color: var(--fg); font-weight: 600; }}
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
{body}
<footer>mojoshelf — an experimental, git-submodule-based registry of reusable Mojo books.</footer>
</body>
</html>"#
    )
}

fn book_table(books: &[BookSummary]) -> String {
    if books.is_empty() {
        return "<p>No books on the shelf yet.</p>".into();
    }
    let rows: String = books
        .iter()
        .map(|b| {
            format!(
                "<tr><td><code>{}</code></td><td>{}</td><td>{}</td>\
                 <td><a href=\"{}\">{}</a></td><td>{}</td></tr>",
                esc(&b.name),
                b.latest_version.as_deref().map(esc).unwrap_or_else(|| "—".into()),
                b.author.as_deref().map(esc).unwrap_or_else(|| "—".into()),
                esc(&b.url),
                esc(&b.url),
                esc(b.description.as_deref().unwrap_or("")),
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
<p>A registry of reusable Mojo books, installed as git submodules.</p>
<h2>Getting started</h2>
<p>Install the <code>shelf</code> CLI:</p>
<pre><code>cargo install --locked --git https://github.com/mojoshelf/mojoshelf mojoshelf</code></pre>
<p>Then, from your project's repo root, add a book from the shelf below:</p>
<pre><code>shelf add &lt;name&gt;</code></pre>
<p>The book and its dependencies land as submodules under <code>shelf/</code>,
pinned to their published commits. See also <code>shelf search</code>,
<code>shelf update</code>, and <code>shelf list</code>.</p>
<h2>Publishing a book</h2>
<p>Add a <code>shelf.toml</code> with your book's <code>name</code>,
<code>version</code>, and dependencies at the repo root, commit and push, then
sign in on the <a href="/authors">Authors</a> tab to generate a publish token.
With <code>SHELF_TOKEN</code> exported, run <code>shelf publish</code> from the
repo root.</p>
<h2>Books</h2>
{}"#,
        book_table(books)
    );
    page("Mojo Shelf", "Books", &body)
}

pub fn authors_signed_out() -> String {
    let body = r#"<h1>Authors</h1>
<p>Sign in with GitHub to publish books, manage your publish token, and
delete versions or books you own.</p>
<p><a href="/auth/login"><button>Sign in with GitHub</button></a></p>"#;
    page("Mojo Shelf authors", "Authors", body)
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
{books_section}"#,
        login = esc(login),
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
