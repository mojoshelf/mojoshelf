# Database and CLI

## Database (D1)

### tins

| field       | type    | notes                             |
| ----------- | ------- | --------------------------------- |
| id          | INTEGER | primary key                       |
| name        | TEXT    | unique, not null                  |
| url         | TEXT    | git clone URL, not null           |
| description | TEXT    |                                   |
| tags        | TEXT    | comma-separated, lowercased       |
| author_id   | INTEGER | FK -> authors; the owner          |
| created_at  | TEXT    | ISO 8601                          |
| updated_at  | TEXT    | ISO 8601                          |

### authors

| field        | type    | notes                                  |
| ------------ | ------- | -------------------------------------- |
| id           | INTEGER | primary key                            |
| github_id    | INTEGER | unique, not null                       |
| github_login | TEXT    | not null                               |
| token_hash   | TEXT    | SHA-256 of the publish token           |
| created_at   | TEXT    | ISO 8601                               |

Authors sign in with GitHub OAuth on the website's Authors tab, where they
generate a publish token (shown once, stored hashed) and can delete versions
of their tins or a whole tin. Deleting is refused while another tin's
published version depends on the tin. Search matches name, description, and
tags. Each tin has a public page with its versions, dependencies, and
dependents; each author has a public page listing their tins.

### versions

| field        | type    | notes                      |
| ------------ | ------- | -------------------------- |
| id           | INTEGER | primary key                |
| tin_id      | INTEGER | FK -> tins                |
| version      | TEXT    | semver; unique per tin    |
| commit_sha   | TEXT    | full 40-char sha, not null |
| published_at | TEXT    | ISO 8601                   |

### dependencies

| field              | type    | notes          |
| ------------------ | ------- | -------------- |
| version_id         | INTEGER | FK -> versions |
| depends_on_tin_id | INTEGER | FK -> tins    |

Primary key: (version_id, depends_on_tin_id). Snapshotted from `shelf.toml`
at publish time. Publishing fails if a dependency name is not a registered
tin. Dependencies are by name only; resolution always picks the dependency's
latest published version.

## shelf.toml

Lives at the tin's repo root:

```toml
name = "lightbug_http"
version = "0.2.0"
description = "HTTP framework for Mojo"
tags = ["http", "networking"]
tins = ["small_time"]
```

`name` and `version` are required; the rest are optional. Authors bump
`version` and commit before publishing. The registry takes the tin's
description and tags from `shelf.toml` on every publish.

## CLI

The binary is `shelf`. Global options:

- `--registry <url>` — registry base URL (default: the deployed instance,
  overridable via `SHELF_REGISTRY`).

### shelf add <name>[@<version>]

Resolves the tin (latest version unless `@<version>` is given) and its full
transitive dependency set from the registry, then adds each as a flat
submodule under `shelf/<name>` pinned to its published commit. Tins already
present are skipped.

- `--dry-run` — print the install set without touching git.

### shelf remove <name>

Removes the tin's submodule. Warns if another installed tin depends on it.

### shelf update [<name>]

Re-pins the named tin (or all installed tins) to its latest published
version, adding any new transitive dependencies.

### shelf list

Lists installed tins with their pinned versions.

### shelf search [term]

Searches registry tin names and descriptions. No term lists all tins.

### shelf info <name>

Shows a tin's description, URL, versions, and dependencies.

### shelf publish

Run from a tin's repo root. Reads `name`, `version`, and dependencies from
`shelf.toml`, takes the current HEAD commit and the `origin` remote URL
(ssh remotes are converted to https), then registers
`(name, version, commit, url, dependencies)` with the registry. The first
publish of a new name registers the tin, owned by the publishing author;
later publishes require the same owner. Fails if the working tree is dirty,
if HEAD is not pushed, or if the version already exists for the tin.
Authenticates with the author's publish token (`SHELF_TOKEN`), obtained on
the website's Authors tab after GitHub sign-in.
