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
- A book is: name, git URL, description, plus published versions
  (version, commit, dependencies).
- The CLI installs books as git submodules under `shelf/<name>`.
- The CLI binary is named `shelf`; the distributed package is `mojoshelf`.
