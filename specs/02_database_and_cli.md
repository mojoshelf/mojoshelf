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
| prev_url    | TEXT    | url before the last URL change    |
| url_changed_at | TEXT | ISO 8601; when url last changed   |
| card        | TEXT    | precomputed agent card (markdown) |
| card_at     | TEXT    | ISO 8601; when card was rebuilt   |
| verified_at | TEXT    | ISO 8601; last tin-smoke report   |
| verified_ok | INTEGER | 1 = smoke build passed everywhere |
| verified_compiler | TEXT | mojo-compiler used (best effort) |
| nightly_at  | TEXT    | ISO 8601; last nightly-channel check |
| nightly_ok  | INTEGER | 1 = nightly smoke build passed    |
| nightly_compiler | TEXT | nightly mojo-compiler (best effort) |

A publish (or admin edit) that changes a tin's `url` records the old URL and
the change time. For 30 days afterwards the site (tin page banner, index
badge) and the CLI (`shelf add`/`update`/`info`) warn that the repo behind
the name changed; the warning then expires automatically.

For agents: the sync cron precomputes a markdown "card" per tin (import
name vs package name, install commands, API surface, usage snippet, health)
served at `/api/tins/<name>/card`, with `/llms.txt` (index) and
`/llms-full.txt` (all cards). The weekly tin-smoke workflow builds every
source tin on linux-64, linux-aarch64, and osx-arm64 against both the
Modular stable and nightly conda channels, and POSTs per-tin, per-channel
outcomes to `/api/verify` (publish-token gated; `"channel": "nightly"`
selects the nightly record), surfaced on tin pages, in `shelf info`, and
in cards. First-party README badges render from the same data:
`/badge/<tin>.svg` (stable verification) and `/badge/<tin>/nightly.svg`
(nightly early warning), plus `/api/tins/<name>/badge[?channel=nightly]`
in the shields.io endpoint schema. An MCP server at `/mcp` (stateless
streamable HTTP, anonymous, read-only) exposes `search_tins`, `tin_info`,
and `usage_example` tools over the same data.

### authors

| field        | type    | notes                                  |
| ------------ | ------- | -------------------------------------- |
| id           | INTEGER | primary key                            |
| github_id    | INTEGER | unique, not null                       |
| github_login | TEXT    | not null                               |
| token_hash   | TEXT    | SHA-256 of the publish token           |
| created_at   | TEXT    | ISO 8601                               |

Tins have a `kind`: `source` (git-pinned, publishable) or `channel` —
modular-community packages mirrored automatically (cron, six-hourly) with
their latest channel version (names differing only by `-` vs `_` are the
same renamed package — only the variant with the newest version is kept); they install as plain conda dependencies in
pixi mode, are rejected in submodule mode, and reserve their names against
publishes.

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

In pixi mode the whole closure is written into pixi.toml's `[dependencies]`
in one edit, entries changed in place, and solved once with `pixi install`.
Solving after each tin would see two revs of a shared dependency half-way
through, and a shim subpackage that kept its name, version and build across
revs becomes two identical source records. If the solve fails, pixi.toml and
pixi.lock are put back as they were. `pixi shelf add` works the same way but
keeps the rev of a tin already in pixi.toml.

### shelf list

Lists installed tins with their pinned versions, and the newer version when
the registry has one. In pixi mode these are the registry tins in pixi.toml's
`[dependencies]`.

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
The registry also reads the tin's pixi manifest at the published commit and
refuses a version that declares a dependency by a path outside the repository,
or that pins a registered tin by git `rev` at a commit the registry has never
published for it (a consumer's solve would see two source records for that
tin). A `rev` is matched as a full sha or a unique prefix of at least 7 hex
characters, since pixi passes it to git; dependencies on the tin's own
repository are skipped. A pin on a published commit that isn't the tin's
latest version is accepted, and the registry answers with a `warnings` list
(`{"ok": true, "warnings": [...]}`) that `shelf publish` prints to stderr;
older CLIs ignore the field. Authenticates with the author's publish token (`SHELF_TOKEN`), obtained on
the website's Authors tab after GitHub sign-in.

### shelf release plan <tin>... | --changed

Run from a directory holding the tins' repositories side by side (each found
by its git URL's last path segment, or `--repo name=path`). Works out the
cascade a change to the named tins ripples up to: every tin whose manifest
pins one in it by `rev`, in any dependency table, recursively, from the
registry's dependents; tins published from a subdirectory go with their
repository's root. Prints, parents first, each repository's release branch,
each tin's current → next version (patch for a pin-only release; for a named
tin `--bump name=patch|minor|major`, or minor/major from `### Breaking` or
`### Removed` under its CHANGELOG's `Unreleased`, or the highest level of its
fragments in `changes/`, which it lists), and every pin that moves,
in manifests and in CI workflows that pin the old commit exactly. Reads each
repository at its fetched default branch; changes no repository, and saves
the plan to `.shelf-plan.json` in the directory it runs from.

- `--changed` — every tin checked out here with non-release commits since its
  published commit (merges and subjects starting `Release` don't count).
- `--without <tin>` — leave a tin out; its dependents keep its published pin.
- `--json` — the plan for scripts, and the input of `apply --from`.

### shelf release apply

Takes the same arguments as `plan` (or `--from <plan.json>`), or none, which
applies the plan `plan` saved in `.shelf-plan.json` (and says so) as long as
every repository's default branch is still at the plan's commit; a stale plan
is refused with a request to re-run `plan`. It carries the plan out: a branch `release/<tin>-<version>` per repository from its
up-to-date default branch, parents first, with the versions, the changelog,
every pin set to its parent's release-branch head, and `pixi lock`; committed
and pushed. Then a preflight (one scratch pixi project adding every release
head, solved and built; `--test` also runs each tin's `test` task), then one
PR per branch via `gh`, labelled `release` and linking the others (without
`gh` or GitHub remotes, it prints the branches). Progress is kept in
`.shelf-release.json`.

- `--resume` — carry on: a release branch that moved (a fix pushed to it) has
  every tin below it re-pinned and relocked with a new commit, and preflight
  rerun for them.
- `--wait` — poll the PRs' checks until they pass, or stop at a failure.
- `--merge` — when every PR is green and every pin is its parent's current
  release head, merge (merge commits, parents first, each checked up to date
  with its base) and publish.
- `--publish` — publish the releases whose branches were merged by hand.
- `--waves` — one level of the graph at a time, each published before the
  next is cut.
- `--skip-preflight`, `--consumer <path>` (print `pixi shelf update` for an app
  afterwards).
- `--verbose` — print preflight's whole build output; by default it goes to
  a log file, with a line per package built and the end of a failed build.

Publishing is `shelf publish` from a detached checkout of each release head,
then a check that the registry shows the version at that commit.

The changelog in a release commit is `Unreleased` moved under the new
version, then the tin's changelog fragments, which the commit deletes.

### shelf changes new <slug> [--bump patch|minor|breaking|major]

Writes a changelog fragment, `changes/<slug>[.<level>].md`, from a template,
in the current directory. One file per PR, so PRs don't conflict over
CHANGELOG.md. The level can also be a first line `bump: minor`; a
`### Breaking` or `### Removed` heading makes it `breaking`; none is patch.
Details in [specs/03_release.md](03_release.md#changelog-fragments).

### shelf changes check [--base <ref>] [--label <name>]

For PR CI: fails when the branch changes code since `--base` (default
`origin/$GITHUB_BASE_REF`, else origin's default branch) without adding or
editing a fragment. Prose-only changes (`*.md`, `docs/`, `LICENSE`, `NOTICE`)
pass, as do PRs labelled `no-changelog` or `release` (labels from
`$GITHUB_EVENT_PATH`, or `--label`). Also fails on a fragment `shelf release`
couldn't read.
