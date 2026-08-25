---
name: mojoshelf-consume
description: Install and use reusable Mojo libraries ("tins") from the mojoshelf registry (mojoshelf.org) as registry-pinned pixi source dependencies or git submodules. Use when a Mojo project needs a third-party library, when the user mentions mojoshelf or `shelf add`, or when building/running code that imports an installed tin.
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

**Pixi mode** (`pixi shelf add <name>`, or `shelf add --pixi <name>`): tins
become registry-pinned git source dependencies in pixi.toml, added flat via
`pixi add --git <url> --rev <commit>` and built by the pixi-build-mojo
backend. Requires `preview = ["pixi-build"]` and channels including conda-forge,
Modular's max channel, and https://repo.prefix.dev/modular-community in the
consumer's `[workspace]`
section (the CLI tells you if it is missing) and requires the tin to be a
pixi package (a `[package]` section in its pixi.toml). If a tin does not
support this yet, fall back to submodule mode.

**Submodule mode** (`shelf add <name>`): works for every tin; details below.

## Find and install a tin

Run from the consuming project's repo root (must be a git repository):

```sh
shelf search <term>       # search names, descriptions, and tags
shelf info <name>         # versions, dependencies, dependents
shelf add <name>          # latest version; or shelf add <name>@<version>
shelf add <name> --dry-run  # preview the install set without touching git
```

`shelf add` resolves the full transitive dependency set in one registry call
and adds every tin — direct and transitive — as a submodule under
`shelf/<name>`, pinned to its published commit. Submodules are never nested.
Commit the resulting `.gitmodules` and submodule changes.

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
