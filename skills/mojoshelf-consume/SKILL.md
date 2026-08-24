---
name: mojoshelf-consume
description: Install and use reusable Mojo libraries ("books") from the mojoshelf registry (mojoshelf.org) as pinned git submodules. Use when a Mojo project needs a third-party library, when the user mentions mojoshelf or `shelf add`, or when building/running code that imports an installed book.
license: MIT
compatibility: Requires git and the shelf CLI (Rust); pixi recommended for the Mojo toolchain
metadata:
  author: mojoshelf
---

# Consume a book from mojoshelf

mojoshelf (https://mojoshelf.org) is a registry of reusable Mojo libraries,
called **books**. Books install as flat git submodules under `shelf/<name>`,
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

**Pixi mode** (`pixi shelf add <name>`, or `shelf add --pixi <name>`): books
become registry-pinned git source dependencies in pixi.toml, added flat via
`pixi add --git <url> --rev <commit>` and built by the pixi-build-mojo
backend. Requires `preview = ["pixi-build"]` in the consumer's `[workspace]`
section (the CLI tells you if it is missing) and requires the book to be a
pixi package (a `[package]` section in its pixi.toml). If a book does not
support this yet, fall back to submodule mode.

**Submodule mode** (`shelf add <name>`): works for every book; details below.

## Find and install a book

Run from the consuming project's repo root (must be a git repository):

```sh
shelf search <term>       # search names, descriptions, and tags
shelf info <name>         # versions, dependencies, dependents
shelf add <name>          # latest version; or shelf add <name>@<version>
shelf add <name> --dry-run  # preview the install set without touching git
```

`shelf add` resolves the full transitive dependency set in one registry call
and adds every book — direct and transitive — as a submodule under
`shelf/<name>`, pinned to its published commit. Submodules are never nested.
Commit the resulting `.gitmodules` and submodule changes.

## Build against installed books

Point the Mojo compiler at each book's `src` with `-I`. With pixi, wrap it
as a task in `pixi.toml`:

```toml
[tasks]
run = "mojo run -I shelf/csv/src src/main.mojo"
```

Then import in Mojo code, e.g. `from csv import parse, read`. Note: with
`mojo run`, `-I` flags must come BEFORE the source file — arguments after
the file are passed to the program.

A complete working example: https://github.com/mojoshelf/example

## Maintain

```sh
shelf list                # installed books with pinned versions
shelf update [<name>]     # re-pin to latest published versions
shelf remove <name>       # remove a book's submodule
```

When cloning a repo that consumes books, use
`git clone --recurse-submodules` (or `git submodule update --init`).
