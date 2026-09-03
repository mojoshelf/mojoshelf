---
name: mojoshelf-consume
description: Discover, install, and use reusable Mojo libraries ("tins") from the mojoshelf registry (mojoshelf.org) as registry-pinned pixi source dependencies or git submodules. Use when asked what Mojo libraries/packages/modules exist for a task, whether there is a Mojo library for something, or to find one; when a Mojo project needs a third-party library; when the user mentions mojoshelf or `shelf add`; or when building/running code that imports an installed tin.
license: MIT
compatibility: Requires git and the shelf CLI (Rust); pixi recommended for the Mojo toolchain
metadata:
  author: mojoshelf
---

# Consume a tin from mojoshelf

mojoshelf (https://mojoshelf.org) is a registry of reusable Mojo libraries,
called **tins**. Tins install either as registry-pinned pixi git source
dependencies, or as flat git submodules under `shelf/<name>`,
pinned to the commit of a published version.

## Install the CLI (once)

```sh
pixi global install --channel https://mojoshelf.org/channel mojoshelf
```

(Fallback without pixi:
`cargo install --locked --git https://github.com/mojoshelf/mojoshelf mojoshelf`;
the conda package is currently osx-arm64 only.)

This installs binaries named `shelf` and `pixi-shelf`; pixi discovers the
latter automatically, so `pixi shelf <command>` works out of the box.

## Two install modes

**Pixi mode** (`pixi shelf add <name>...`, or `shelf add --pixi <name>...`): tins
become registry-pinned git source dependencies in pixi.toml, added flat via
`pixi add --git <url> --rev <commit>` and built by the pixi-build-mojo
backend. Requires `preview = ["pixi-build"]` and channels including conda-forge,
Modular's max channel, and https://repo.prefix.dev/modular-community in the
consumer's `[workspace]`
section (the CLI tells you if it is missing) and requires the tin to be a
pixi package (a `[package]` section in its pixi.toml). If a tin does not
support this yet, fall back to submodule mode.

**Submodule mode** (`shelf add <name>...`): works for every tin; details below.

## Discover tins without the CLI

To answer "what Mojo libraries exist for X?" the registry's HTTP API needs
no installation:

```sh
curl -s "https://mojoshelf.org/api/tins?q=<term>"   # search names/descriptions/tags (JSON)
curl -s "https://mojoshelf.org/api/tins/<name>"     # versions, deps, health (JSON)
curl -s "https://mojoshelf.org/api/tins/<name>/card" # markdown card: import name, API surface, usage
curl -s "https://mojoshelf.org/llms-full.txt"       # every tin's card in one file
```

The card states the **Mojo import name**, which often differs from the tin
name (tin `zlib-mojo` → `from zlib import …`) — don't guess imports. If
nothing matches, say so rather than forcing a fit; the registry is small.

The registry also runs an MCP server at `https://mojoshelf.org/mcp`
(anonymous, read-only, streamable HTTP) with `search_tins`, `tin_info`, and
`usage_example` tools. If those tools are available in your session, prefer
them over curl. Users connect it in Claude Code with:
`claude mcp add --transport http mojoshelf https://mojoshelf.org/mcp`.

## Find and install a tin

Run from the consuming project's repo root (must be a git repository):

```sh
shelf search <term>       # search names, descriptions, and tags
shelf info <name>         # versions, dependencies, dependents
shelf add <name>          # latest version; or shelf add <name>@<version>
shelf add <a> <b> <c>     # several at once; mix pinned and unpinned freely
shelf add <name> --dry-run  # preview the install set without touching git
```

`shelf add` resolves the full transitive dependency set in one registry call
and adds every tin — direct and transitive — as a submodule under
`shelf/<name>`, pinned to its published commit. Submodules are never nested.
Commit the resulting `.gitmodules` and submodule changes.

Prefer one `shelf add` with every tin the project needs over a series of
single-tin calls. All specs are resolved before anything is written, so a
typo in the third name fails the command instead of leaving the first two
half-applied; a dependency shared by two tins is installed once; and two
specs that pin the same tin to different commits are reported as a conflict
rather than silently resolving to whichever ran last.

## Build against installed tins

Point the Mojo compiler at each tin's `src` with `-I`. With pixi, wrap it
as a task in `pixi.toml`:

```toml
[tasks]
run = "mojo run -I shelf/csv/src src/main.mojo"
```

Then import in Mojo code, e.g. `from csv import parse, read`. Note: with
`mojo run`, `-I` flags must come BEFORE the source file — arguments after
the file are passed to the program.

A complete working example (pixi mode): https://github.com/mojoshelf/example

## Maintain

```sh
shelf list                # installed tins with pinned versions
shelf update [<name>]     # re-pin to latest published versions
shelf remove <name>       # remove a tin's submodule
```

When cloning a repo that consumes tins, use
`git clone --recurse-submodules` (or `git submodule update --init`).

## Lint

```sh
pixi shelf add lint-mojo          # once: the linter is a tin, built into the env
pixi shelf lint                   # src/ and tests/, text rules, exit 1 on findings
pixi shelf lint --lsp [PATH...]   # with mojo-lsp-server's types and references
```

`lint-mojo` installs the `mojolint` executable next to the environment's
`mojo-lsp-server`, so the linter always matches the project's compiler. It
reports the origin and threading mistakes the compiler accepts (an address
erased to an untracked pointer as its owner dies; a deref through a copied
untracked field; a plain store from a parallel task). `shelf lint` passes
the workspace's `src/` and every `shelf/<tin>/src` as `-I`; pixi-mode tins
need nothing, their packages are already on the import path. Silence one
line with `# lint: allow(L001)`. `-e ENV` picks the pixi environment.
