---
name: mojoshelf-publish
description: Publish a Mojo library ("tin") to the mojoshelf registry (mojoshelf.org) with shelf publish. Use when the user wants to publish, release, or version a Mojo library on mojoshelf, or needs a shelf.toml manifest created or updated.
license: MIT
compatibility: Requires git and the shelf CLI (Rust); publishing needs a SHELF_TOKEN
metadata:
  author: mojoshelf
---

# Publish a tin to mojoshelf

mojoshelf (https://mojoshelf.org) is a registry of reusable Mojo libraries,
called **tins**. Publishing registers `(name, version, commit, url,
description, tags, dependencies)`; consumers then install the tin as a git
submodule pinned to that commit.

## Prerequisites

1. The `shelf` CLI:
   `pixi global install --channel https://mojoshelf.org/channel mojoshelf`
   (or `cargo install --locked --git https://github.com/mojoshelf/mojoshelf mojoshelf`)
2. A publish token in `SHELF_TOKEN`. The user gets one by signing in with
   GitHub at https://mojoshelf.org/authors and clicking "Generate publish
   token". Never print or log the token; if it is missing, ask the user to
   set it — do not guess.
3. The tin's repo must have a public `origin` remote (ssh remotes are
   converted to https automatically).

## shelf.toml

The manifest lives at the tin's repo root:

```toml
name = "lightbug_http"          # required; [a-z0-9_-] only
version = "0.2.0"               # required; semver, bumped for every publish
description = "HTTP framework for Mojo"   # optional
tags = ["http", "networking"]             # optional, lowercased
tins = ["small_time"]          # optional: mojoshelf tins this one depends on
```

The registry snapshots description, tags, and dependencies from `shelf.toml`
on every publish. Dependencies must already be registered tins — publish
bottom-up (dependencies first). Verify with `shelf info <dep>`.

## Publish

From the tin's repo root:

```sh
shelf publish
```

The CLI takes the current HEAD commit and refuses to publish if:

- the working tree is dirty → commit or stash first
- HEAD is not on any remote branch → push first
- the version already exists → bump `version` in shelf.toml, commit, push
- a dependency is not a registered tin → publish the dependency first

The first publish of a new name registers the tin, owned by the publishing
author; later publishes must come from the same author (registry answers
403 otherwise).

It also refuses a version whose manifest declares a dependency by a path
outside the repository (`{ path = "../sibling" }`), because that resolves in
your checkout and nobody else's.

## One repo, more than one tin

A repository can publish several tins — useful when an optional half of a
library pulls in heavy dependencies and you want them optional at *install*
time, not just at compile time. Give the second tin its own subdirectory
holding a `pixi.toml`, a `shelf.toml`, and its sources:

```
parquet.mojo/
  pixi.toml  shelf.toml      parquet-mojo
  src/parquet/
  full/
    pixi.toml  shelf.toml    parquet-full-mojo
    src/parquet_full/        sources live under the subdirectory
```

Two things to get right:

- A subdirectory manifest's build context is **the subdirectory**, so
  `[package.build.config.pkg] path` is relative to it (`src/parquet_full`
  means `full/src/parquet_full`).
- Depend on the root package by relative path — `parquet-mojo = { path = ".." }`
  in `[package.host-dependencies]`. This is inside the repository, so it is
  allowed, and it always means the commit being built: the two tins move
  together and never need re-pinning against each other.

Publish it by running `shelf publish` **from that subdirectory**. Where the
manifest sits is the whole declaration — nothing in `shelf.toml` repeats it —
and the registry hands consumers the right `subdirectory` automatically:

```sh
cd full && shelf publish        # "published parquet-full-mojo 0.1.0 (abc123…) from full/"
pixi shelf add parquet-full-mojo
# writes { git = "…", rev = "…", subdirectory = "full" }
```

Publish bottom-up here too: the root tin first, then the subdirectory one.

## Typical flow

```sh
# 1. edit shelf.toml (bump version, adjust description/tags/tins)
# 2. commit and push everything
git add -A && git commit -m "Release 0.2.0" && git push
# 3. publish
shelf publish
# 4. verify
shelf info <name>
```

## Graduating to the modular-community channel

When a tin is stable, `shelf graduate` (from the tin's repo root) generates
a channel-ready rattler-build `recipe.yaml` — preflight-checked, source
pinned to the pushed commit, license and maintainer detected
(`--maintainer` / `--license` to override) — and prints the fork-and-PR
submission steps for github.com/modular/modular-community. Dependencies on
other tins must already exist on the channel; FFI shims need hand-porting
into the recipe (the command warns).
