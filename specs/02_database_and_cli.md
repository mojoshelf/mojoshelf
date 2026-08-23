# Database and CLI

## Database (D1)

### books

| field       | type    | notes                   |
| ----------- | ------- | ----------------------- |
| id          | INTEGER | primary key             |
| name        | TEXT    | unique, not null        |
| url         | TEXT    | git clone URL, not null |
| description | TEXT    |                         |
| created_at  | TEXT    | ISO 8601                |
| updated_at  | TEXT    | ISO 8601                |

### versions

| field        | type    | notes                      |
| ------------ | ------- | -------------------------- |
| id           | INTEGER | primary key                |
| book_id      | INTEGER | FK -> books                |
| version      | TEXT    | semver; unique per book    |
| commit_sha   | TEXT    | full 40-char sha, not null |
| published_at | TEXT    | ISO 8601                   |

### dependencies

| field              | type    | notes          |
| ------------------ | ------- | -------------- |
| version_id         | INTEGER | FK -> versions |
| depends_on_book_id | INTEGER | FK -> books    |

Primary key: (version_id, depends_on_book_id). Snapshotted from `shelf.toml`
at publish time. Publishing fails if a dependency name is not a registered
book. Dependencies are by name only; resolution always picks the dependency's
latest published version.

## shelf.toml

Lives at the book's repo root:

```toml
name = "lightbug_http"
version = "0.2.0"
books = ["small_time"]
```

A book with no dependencies may omit `books`. `name` and `version` are
required; authors bump `version` and commit before publishing.

## CLI

The binary is `shelf`. Global options:

- `--registry <url>` — registry base URL (default: the deployed instance,
  overridable via `SHELF_REGISTRY`).

### shelf add <name>[@<version>]

Resolves the book (latest version unless `@<version>` is given) and its full
transitive dependency set from the registry, then adds each as a flat
submodule under `shelf/<name>` pinned to its published commit. Books already
present are skipped.

- `--dry-run` — print the install set without touching git.

### shelf remove <name>

Removes the book's submodule. Warns if another installed book depends on it.

### shelf update [<name>]

Re-pins the named book (or all installed books) to its latest published
version, adding any new transitive dependencies.

### shelf list

Lists installed books with their pinned versions.

### shelf search [term]

Searches registry book names and descriptions. No term lists all books.

### shelf info <name>

Shows a book's description, URL, versions, and dependencies.

### shelf publish

Run from a book's repo root. Reads `name`, `version`, and dependencies from
`shelf.toml` and takes the current HEAD commit, then registers
`(name, version, commit, dependencies)` with the registry. Fails if the
working tree is dirty, if HEAD is not pushed, if the version already exists
for the book, or if the book is not already registered by an admin.
Authenticates with a Cloudflare Access service token (`SHELF_CLIENT_ID` /
`SHELF_CLIENT_SECRET`). Authors receive their service-token credentials from
an admin; any credentialed author can publish any book.
