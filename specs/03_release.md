# Releasing several tins at once

Status: implemented in the CLI (`shelf release plan` and `apply`,
`crates/mojoshelf/src/release/`), with these differences:

- `--resume` handles a parent whose release branch moved; a default branch
  that moved under a release branch isn't re-cut (that would need a force
  push). `--merge` stops and asks for the base to be merged into the release
  branch as a new commit, which `--resume` then treats as a moved head.
- `--publish` publishes releases whose PRs were merged by hand; `--merge`
  publishes after merging.
- `--without` doesn't check that dependents build against the dropped tin's
  published version; preflight does.
- `--changed` counts non-merge commits whose subject doesn't start with
  `Release`. Changelog fragments and consumer PRs (open questions below) are
  not done.

## The problem

A tin that a sibling tin builds against pins it by git revision in its
package manifest:

```toml
[package.host-dependencies]
deflate-mojo = { git = "https://github.com/magmalake/deflate.mojo", rev = "6639b18…" }
```

That revision must be the commit the registry published for the
dependency's latest version. Any other revision and a consumer's solve sees
two source records for one package (`encountered duplicate records for
…_source.conda`). So a change to one tin ripples up its dependents: each one
needs a release that does nothing but move the pin, published bottom-up,
and each release can only be prepared once the one below it is published,
because only then is its commit known.

For example, a change to deflate in magmalake (deflate → avro → parquet →
iceberg → orca, plus a consumer app) is five releases in a row. Each waits
for the one before it to be reviewed, run through CI, merged and published.
One three-tin cascade on 2026-11-05 took about an hour, about 31 minutes of
which was three release PRs' CI running one after another.

Nothing in the registry or the CLI helps today. Authors work out the order
from `shelf info`, edit the pins by hand in every place a repository keeps
them, and publish one at a time. A pin that's wrong is found when a
consumer's solve fails.

## Goals

- One command plans a cascade and another runs it: the tins to release, the
  versions, every pin that moves, in order.
- Every release in a cascade can be prepared, and reviewed and tested, at
  the same time. Waiting is one CI run, not one per tin.
- What's published is what was tested: each tin is published at the commit
  its release was tested at.
- A pin that disagrees with the registry is caught at publish time, not by
  a consumer.

Not goals: changing how consumers resolve (`shelf add` stays as it is), or
running anyone's CI.

## Design

### shelf release plan <tin>... | --changed

Run from a directory holding the tins' repositories side by side (each found
by its git URL's last path segment, or `--repo name=path`).

1. **Which tins.** The named tins, or with `--changed` every source tin whose
   repository has commits on its default branch since its published commit
   (release commits, which change only versions, changelogs and pins, don't
   count).
2. **The cascade.** Add every tin whose manifest pins one of them by `rev`, in
   any dependency table (`[dependencies]`, `[package.host-dependencies]`,
   `[package.run-dependencies]`, `[package.build-dependencies]`, and target-
   or feature-specific ones), recursively. The registry already has
   dependents (`/api/tins/<name>` lists them); the manifests say which are
   pinned by `rev` and so need a release. A dependent that pins nothing in the
   cascade isn't touched.
3. **Versions.** Patch for a tin that's only in the cascade to move pins. For
   a named tin, `--bump name=minor|patch|major` (default patch), or from its
   changelog's `Unreleased` headings if it has one (`### Breaking` or
   `### Removed` → minor below 1.0, major after).
4. **Print the plan**: tin, current → next version, why it's in, and each pin
   it moves (file, table, old → new rev). `plan` changes nothing. `--json`
   prints it for scripts.

Tins in a subdirectory of a repository (`full/`) are released with their root
at the same commit, as `shelf publish` already requires; they don't need
their own pin moves, since they reach the root by `path = ".."`.

### shelf release apply [--from <plan.json>]

1. **Branches, all at once.** For each repository in the cascade, from its
   up-to-date default branch, a branch `release/<tin>-<version>`:
   - `version` set in `shelf.toml`, and in the manifest's `[package]` and
     `[workspace]` where present;
   - `Unreleased` in `CHANGELOG.md`, if there is one, moved under the new
     version and today's date;
   - each pin to a tin in the cascade set to **the head commit of that tin's
     release branch**, wherever the repository pins it;
   - `pixi lock`, so the lock file agrees;
   - committed and pushed.

   Parents' branches are made first, so their heads are known before the
   children pin them. Nothing waits on anything else.
2. **Preflight**, before any PR: one scratch project that adds every release
   head the way a consumer would (`pixi shelf add` of each, in one edit), solves
   and builds. That's a minute or two, and it catches the failures a release
   PR can have that aren't in the code: a wrong pin, two revisions of one tin,
   a lock file that disagrees. With `--test`, it also runs each tin's own tests
   against the others' release heads. If preflight fails, nothing is opened.
3. **Pull requests.** With `gh` available, one PR per branch, labelled
   `release`, each body listing the whole cascade with links to its sibling
   PRs. Without `gh`, it prints the branches.
4. **Wait** (optional, `--wait`): poll the PRs' required checks. If one fails,
   stop and say which; nothing is merged or published.
5. **Merge** (optional, `--merge`, needs `gh`): only when every PR in the
   cascade is green **and** every pin in every release branch equals its
   parent's current release head. Then parents first, with merge commits.
   Before each, check the branch is still up to date with its base;
   if the base moved, stop and say so (rerun `apply`, which re-cuts that
   branch and its descendants).
6. **Publish**, parents first: a detached checkout of each release branch's
   head commit (which its base now contains), then the same steps as
   `shelf publish`, and for a repository with a subdirectory tin, that too
   from the same commit. Then check the registry shows each version at that
   commit.

`apply` keeps its state (branches, head commits, PRs, what's published) in
`.shelf-release.json` beside the repositories, so a run that stops can be
resumed: `shelf release apply --resume`.

Publishing at the release branch's head, not the merge commit, is what lets
everything happen at once. `shelf publish` already accepts any pushed commit,
and the head commit is in the default branch after a merge-commit merge, with
the same tree when the branch was up to date (step 4 checks this). Repositories
that squash or rebase on merge get a new commit with the same tree; `apply`
checks the tree and publishes the head anyway, but says the published commit
isn't on the default branch.

### When a release PR fails

A release's diff is versions, a changelog and pins, so a failing release PR is
one of three things:

- **The cascade itself is wrong** (a pin, a duplicate record, a lock file).
  Preflight catches this before any PR is opened.
- **A parent's change breaks a child**, for instance an API the child calls.
  That child needs a code change, which isn't a release's job: it goes in a
  PR of its own against the child's default branch, and the child's release
  is cut again once it's merged. Preflight with `--test` finds most of these
  too.
- **A flaky check.** Rerunning it changes no commit, so nothing else moves.

When a parent's release branch does have to change, the fix goes on that
branch as a new commit (no force push), and `apply --resume`:

1. notices the parent's head moved;
2. for every tin below it in the cascade, and only those, adds a commit that
   moves the pin to the new head, and relocks;
3. reruns preflight for that subtree, and waits for those PRs' checks again.

Tins outside the subtree keep their branches and their green checks. The cost
of a failure is its fix plus one more round of CI for the tins below it.

The merge rule (every pin equal to its parent's current head) is what makes
this safe: a child tested against a parent commit that's since been replaced
can't merge, so nothing is published against a commit that isn't.

Two ways out when a fix isn't coming soon:

- `shelf release plan --without <tin>` recomputes the cascade without it; its
  dependents go back to pinning its latest published version. `plan` refuses
  when a dependent needs the dropped tin's unreleased code (it fails to build
  against the published one in preflight).
- `--waves` releases one level of the graph at a time, publishing each before
  cutting the next: today's serial order, for a cascade that keeps failing.

### A pin check at publish time

Status: implemented in the registry (`pin_check` in
`crates/shelf-worker/src/manifest.rs`); `shelf publish` prints its warnings.
There is no client-side copy, since the registry answers before it records
anything. `scripts/pin-dryrun.sh` runs it over published manifests.

`shelf publish` (and the registry, for publishes from older CLIs) reads the
manifest at the commit being published and, for every dependency that pins a
registered tin by `rev`, checks that the rev is a commit the registry has
published for it, and warns when it isn't that tin's latest. A rev the
registry has never published is an error:

```
deflate-mojo is pinned at 1a2b3c4 in [package.host-dependencies], which isn't a
published version; its latest is 0.2.0 at 6639b18. Consumers will see two
source records for deflate-mojo.
```

It sits beside the check that already refuses a dependency by a path outside
the repository (`crates/shelf-worker/src/manifest.rs`), which reads the same
manifest at the same commit. Releases made by `shelf release` pass it by
construction; hand-made ones get the same safety.

### Consumers (apps)

Apps that pin tins aren't tins themselves, so they aren't in the cascade.
`pixi shelf update` already re-pins a consumer's whole closure in one edit.
After `apply` it prints that as the next step for any consumer directory given
with `--consumer <path>`.

## Later: fewer pin-only releases

Most releases in a cascade exist only to move a pin, and they exist because
package dependencies on sibling tins are git revisions fixed at commit time.
Two ways to drop them, both outside this proposal:

- **Resolve the pins at install time.** If pixi could override a source
  package's own source dependencies from the top-level manifest, `shelf add`
  (which already writes the whole closure in one edit) could make every tin
  in a solve use the revisions it chose, and tins could leave sibling pins
  out of their manifests. That needs a pixi feature; worth raising upstream.
- **Build tins into a channel.** If the registry built each published
  version into conda packages on the mojoshelf channel, tins could depend on
  each other by name and version range (`deflate-mojo >=0.2,<0.3`) and a
  patch release below wouldn't touch anything above it. It's a much bigger
  step: a build farm, per-platform artefacts, and the `.mojoc` compiler-version
  lockstep.

## Open questions

- Should `release` also open the consumer apps' PRs (`--consumer`), or leave
  that to `pixi shelf update`?
- Changelog fragments (one file per change under `changes/`, gathered at
  release) would end changelog conflicts when several PRs land together.
  Support them now, or only `Unreleased`?
- `--changed` needs to tell release commits from others. By subject
  (`Release x.y.z`), by the files touched, or by a trailer?
