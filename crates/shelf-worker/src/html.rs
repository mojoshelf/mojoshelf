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
  body {{ font-family: ui-sans-serif, system-ui, sans-serif; max-width: 52rem;
         margin: 2rem auto; padding: 0 1rem; color: #1a1a1a; }}
  h1 {{ font-size: 1.5rem; }}
  nav {{ margin-bottom: 1.5rem; border-bottom: 1px solid #ddd; }}
  nav a {{ display: inline-block; padding: .4rem .8rem; text-decoration: none;
          color: #444; }}
  nav a.active {{ border-bottom: 2px solid #e44d26; color: #1a1a1a; font-weight: 600; }}
  table {{ border-collapse: collapse; width: 100%; }}
  th, td {{ text-align: left; padding: .4rem .6rem; border-bottom: 1px solid #ddd; }}
  code {{ background: #f4f4f4; padding: .1rem .3rem; border-radius: 3px; }}
  form.book {{ margin: 1rem 0; padding: 1rem; border: 1px solid #ddd; border-radius: 6px; }}
  form.inline {{ display: inline; }}
  input, textarea {{ width: 100%; box-sizing: border-box; margin: .2rem 0 .6rem; padding: .3rem; }}
  button.danger {{ background: #fee; border: 1px solid #c66; color: #900;
                  border-radius: 4px; cursor: pointer; }}
  .token {{ background: #fff8e0; border: 1px solid #e0c860; padding: 1rem;
           border-radius: 6px; word-break: break-all; }}
  footer {{ margin-top: 2rem; font-size: .8rem; color: #777; }}
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
        "<h1>mojoshelf</h1>\
         <p>Install a book with <code>shelf add &lt;name&gt;</code>. \
         API: <a href=\"/api/books\">/api/books</a></p>{}",
        book_table(books)
    );
    page("mojoshelf", "Books", &body)
}

pub fn authors_signed_out() -> String {
    let body = r#"<h1>Authors</h1>
<p>Sign in with GitHub to publish books, manage your publish token, and
delete versions or books you own.</p>
<p><a href="/auth/login"><button>Sign in with GitHub</button></a></p>"#;
    page("mojoshelf authors", "Authors", body)
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
    page("mojoshelf authors", "Authors", &body)
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
        r#"<h1>mojoshelf admin</h1>
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
    page("mojoshelf admin", "Books", &body)
}
