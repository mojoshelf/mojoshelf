# MojoRegistry

## Intro

The Mojo Registry is a helper tool, website and database to track re-usable Mojo
packages.

Since Mojo packages are not defined yet by Modular the current approach is to
use git submodules.

This code will be deprecated once Modular supports packages.

## Architecture

In this system the packages will be called "books" to leave
the most freedom for the official Mojo Registry.

The code will be comprised of:

1. A Database defining packages
2. A Website with authentication allowing admin users to add or edit packages and any
   user to download information about packages.
3. A CLI tool allowing users to add packages using git submodules.

The code will be deployed at Cloudflare and use Rust as a language.

### Versions and dependencies

A book declares its dependencies (book names only) in a `shelf.toml` file at
its repo root. A commit becomes visible to consumers only when a version is
published: `shelf publish`, run from the book's repo, registers
`(book, version, commit)` and submits the dependencies from `shelf.toml` at
that commit.

Because the registry snapshots dependencies at publish time, `shelf add`
resolves the full transitive set with one API call, then installs every book —
direct and transitive — as a flat submodule of the top-level repo under
`shelf/<name>`, pinned to its published commit. Submodules are never nested;
cycles terminate via a visited set.

## Decisions

- Database: Cloudflare D1.
- Admin auth: Cloudflare Access gates the admin routes; public routes are open.
- Authors self-register with GitHub OAuth on the website's Authors tab and get
  a registry-issued publish token. The first publish of a name registers the
  book, owned by the publisher; owners can delete versions or whole books.
- A book is: name, git URL, description, plus published versions
  (version, commit, dependencies).
- The CLI installs books as git submodules under `shelf/<name>` (submodule
  mode), or — as `pixi shelf` / with `--pixi` — as registry-pinned git source
  dependencies written via `pixi add --git`, flattened like submodule mode
  and built by pixi-build-mojo (requires the pixi-build preview and a
  `[package]` section in the book).
- The CLI binary is named `shelf` and also installs as `pixi-shelf`, pixi's
  extension convention for the `pixi shelf` subcommand; the distributed
  package is `mojoshelf`, shipped as a conda package on a static channel at
  mojoshelf.org/channel (Worker assets) for `pixi global install`, with
  `cargo install` from the repo as the fallback.
