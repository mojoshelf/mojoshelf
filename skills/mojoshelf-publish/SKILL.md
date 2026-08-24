---
name: mojoshelf-publish
description: Publish a Mojo library ("book") to the mojoshelf registry (mojoshelf.org) with shelf publish. Use when the user wants to publish, release, or version a Mojo library on mojoshelf, or needs a shelf.toml manifest created or updated.
license: MIT
compatibility: Requires git and the shelf CLI (Rust); publishing needs a SHELF_TOKEN
metadata:
  author: mojoshelf
---

# Publish a book to mojoshelf

mojoshelf (https://mojoshelf.org) is a registry of reusable Mojo libraries,
called **books**. Publishing registers `(name, version, commit, url,
description, tags, dependencies)`; consumers then install the book as a git
submodule pinned to that commit.

## Prerequisites

1. The `shelf` CLI:
   `cargo install --locked --git https://github.com/mojoshelf/mojoshelf mojoshelf`
2. A publish token in `SHELF_TOKEN`. The user gets one by signing in with
   GitHub at https://mojoshelf.org/authors and clicking "Generate publish
   token". Never print or log the token; if it is missing, ask the user to
   set it — do not guess.
3. The book's repo must have a public `origin` remote (ssh remotes are
   converted to https automatically).

## shelf.toml

The manifest lives at the book's repo root:

```toml
name = "lightbug_http"          # required; [a-z0-9_-] only
version = "0.2.0"               # required; semver, bumped for every publish
description = "HTTP framework for Mojo"   # optional
tags = ["http", "networking"]             # optional, lowercased
books = ["small_time"]          # optional: mojoshelf books this one depends on
```

The registry snapshots description, tags, and dependencies from `shelf.toml`
on every publish. Dependencies must already be registered books — publish
bottom-up (dependencies first). Verify with `shelf info <dep>`.

## Publish

From the book's repo root:

```sh
shelf publish
```

The CLI takes the current HEAD commit and refuses to publish if:

- the working tree is dirty → commit or stash first
- HEAD is not on any remote branch → push first
- the version already exists → bump `version` in shelf.toml, commit, push
- a dependency is not a registered book → publish the dependency first

The first publish of a new name registers the book, owned by the publishing
author; later publishes must come from the same author (registry answers
403 otherwise).

## Typical flow

```sh
# 1. edit shelf.toml (bump version, adjust description/tags/books)
# 2. commit and push everything
git add -A && git commit -m "Release 0.2.0" && git push
# 3. publish
shelf publish
# 4. verify
shelf info <name>
```
