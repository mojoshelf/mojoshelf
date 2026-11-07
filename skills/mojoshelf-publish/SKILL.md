---
name: mojoshelf-publish
description: Publish a Mojo library ("tin") to the mojoshelf registry (mojoshelf.org) with shelf publish. Use when the user wants to publish, release, or version a Mojo library on mojoshelf, release several tins that depend on each other, or needs a shelf.toml manifest created or updated.
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

It also refuses a version that pins another registered tin at a commit the
registry never published for it (`deflate-mojo = { git = "...", rev = ".." }`),
because a consumer's solve then sees two source records for that tin. Pin the
commit `shelf info <tin>` lists for its latest version. A pin on an older
published version is accepted with a warning.

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

Publish bottom-up here too: the root tin first, then the subdirectory one —
**in the same session, from the same commit.**

Two rules, each of which ships an uninstallable tin when broken:

- **Every tin in the repo publishes at the same commit.** `path = ".."`
  resolves to `git+<repo>?rev=<the commit the subdirectory tin was published
  at>`. If the registry pins the root at a different commit, consumers get two
  source records for one package and `pixi shelf add` fails with `encountered
  duplicate records for <root>-…_source.conda`.
- **Do not list an in-repo path dependency in `tins`.** That field is for tins
  the registry resolves. The root is already reached through `path = ".."`;
  naming it there pins it a second time and causes the same collision. A tin's
  own FFI shim is not listed either.

Also make sure the `shelf` CLI is new enough to know about subdirectories
(0.5.0+). An older CLI publishes with no subdirectory recorded, and the install
fails with *"the package '<name>' is not provided by the project located at
git+…"* — which reads like a packaging bug in the tin rather than version skew
in the publisher. Check what the registry actually stored:

```sh
curl -s https://mojoshelf.org/api/tins/<name> | grep -o '"subdirectory":[^,]*'
```

## Releasing tins that pin each other

A tin that another tin builds against is usually pinned by revision in the
consumer's package manifest:

```toml
[package.host-dependencies]
deflate-mojo = { git = "https://github.com/magmalake/deflate.mojo", rev = "6639b18…" }
```

That `rev` must be **the commit the registry published** for the
dependency's version. Any other commit, even one with the same code, and a
consumer's solve sees two source records for one package and fails with
`encountered duplicate records for …_source.conda`. So a release of one tin
means a release of every tin that pins it, only to move the pin, and of
everything that pins those, published bottom-up.

With shelf 0.6 or later, two commands do it, run from a directory holding
the repositories side by side:

```sh
shelf release plan deflate-mojo        # the cascade, versions, every pin that moves
shelf release apply deflate-mojo       # branches, preflight, PRs (gh)
shelf release apply --resume --merge   # once green: merge and publish, parents first
```

`apply` cuts every release branch at once, each pinning its parents' release
heads, builds them together before opening any PR, and keeps its progress in
`.shelf-release.json`. A fix pushed to a parent's release branch is picked up
by `apply --resume`, which re-pins only what's below it. After 0.6.0, CI checkouts of any
repository below a tin in the cascade move with its pins. `plan` also reports
a sibling outside the cascade that a tin pins or checks out at another commit
than its parents pin it at, and `apply` refuses that unless
`--allow-mismatch`. `shelf release --help` has the rest (`--changed`,
`--bump`, `--without`, `--waves`, `--test`).

Changelog entries can be fragments instead of one `## Unreleased` section
(shelf 0.8+): each PR adds `changes/<pr>-<slug>.md` (`shelf changes new
155-fills --bump minor` writes `changes/155-fills.minor.md`), and
`shelf release apply` writes them under the new version in CHANGELOG.md and
deletes them. The level is the file name's, or a first line `bump: minor`;
`### Breaking` makes it breaking; the tin's bump is the highest. Keep a
`changes/README.md` so the directory outlives a release, and run
`shelf changes check` in PR CI (checkout with `fetch-depth: 0`); a PR labelled
`no-changelog` skips it. See
[specs/03_release.md](../../specs/03_release.md#changelog-fragments).

With an older CLI, or to do it by hand:

1. **Find the cascade.** `shelf info <tin>` lists its dependents
   (`curl -s https://mojoshelf.org/api/tins/<tin>` has them as
   `dependents`). For each, look in its manifest for a `rev` pin to the tin, in
   every table that can hold one: `[dependencies]`,
   `[package.host-dependencies]`, `[package.run-dependencies]`,
   `[package.build-dependencies]`, and feature- or target-specific ones. CI
   workflows that check siblings out by commit pin them too. A dependent with
   no `rev` pin needs no release. Repeat for each dependent that does.
2. **Release from the bottom.** For the lowest tin: bump `version`, commit,
   push, `shelf publish`. Then read back the commit it was published at:

   ```sh
   curl -s https://mojoshelf.org/api/tins/<tin> | grep -o '"commit_sha":"[0-9a-f]*"' | head -1
   ```

3. **Move the pins one level up** to exactly that commit, in every place the
   consumer pins it, then `pixi lock`, bump its version, commit, push, publish,
   and read back its commit. Repeat level by level.
4. **Check each publish** with `shelf info <tin>`, and finish with
   `pixi shelf update` in the apps that use the tins.

Mistakes to avoid:

- Pinning a dependency's branch head or merge commit instead of the commit
  the registry recorded. Always read it back from the registry.
- Leaving a pin behind in one table, or in CI. The build may still pass,
  because a sibling's source on the include path hides the stale package, and
  then a consumer's solve fails.
- Publishing a level before the one below it is published, or two tins in
  one repo at different commits (see "One repo, more than one tin").

The design behind `shelf release` is in
[specs/03_release.md](../../specs/03_release.md).

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
