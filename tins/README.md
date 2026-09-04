# tins

Release plumbing for a polyrepo of [mojoshelf](https://mojoshelf.org) tins.

A tin is one repository: its own version, its own CI, its own entry in the
registry, and dependencies on sibling tins pinned by git revision. That
shape is good for consumers and tedious for whoever maintains a family of
them — the `magmalake` stack is twenty repos, `millfolio` another ten.
Three jobs come up over and over:

- **Fan-out.** The same edit in fifteen repos — a CI bump, a re-pin — each
  needing a worktree, a lock, a commit, a push and a PR.
- **Release order.** Publishing bottom-up, because a tin that pins an
  unpublished revision of a sibling breaks every install.
- **The invariants.** The rules that are obvious once you have been bitten
  and invisible until then. Most of this tool is those rules.

`tins` does those three things and nothing else.

### Why not a monorepo task runner

moon, turbo and friends build a task graph inside *one* repository. Nothing
here is one repository, and the expensive part is not running tasks —
`pixi run test` already does that, per repo, and does it well. The expensive
part is the choreography between repos and the rules below, which no
general-purpose runner knows. If the tins ever became a monorepo this tool
would be the wrong answer and a task runner would be the right one.

## Install

```sh
git clone https://github.com/mojoshelf/mojoshelf
cd mojoshelf/tins
cp tins.example.toml tins.toml   # your roots and identity
pixi run tins doctor
```

There is nothing to build. The tool is Python 3.12 with no third-party
dependencies; pixi is only there to supply an interpreter new enough to
have `tomllib`. This directory is its own pixi workspace, separate from
the CLI's at the repo root, so run pixi from here. To call it from
anywhere, put `bin/tins` on your `PATH`.

## Configure

`tins.toml` says where the checkouts are and who commits. It is not in git
— roots and identity are per-person — so copy `tins.example.toml` first:

```toml
[defaults]
registry = "https://mojoshelf.org"
pixi = "0.78.0"
author = "Marius S <39998+winding-lines@users.noreply.github.com>"
co_authored_by = "Claude Fable 5 <noreply@anthropic.com>"

[discovery]
roots = ["~/dev", "~/dev/magmalake", "~/dev/labelrefinery"]
ignore = ["mojoshelf", "modular"]
```

Each root's immediate subdirectories are scanned for git clones with a
`pixi.toml`. **A repo's org comes from its git remote, not from the folder
it sits in** — the clones are not grouped by org on disk, and two roots may
hold the same remote. Linked worktrees are skipped, and when one remote is
cloned twice only one checkout is used.

Point `--config` or `TINS_CONFIG` at another file to work on a different
set of repos.

## Commands

Every command takes `--org`, `--repo` (repeatable) and `--tins` to narrow
what it touches.

### `tins list`

One line per repo: local version, published version, pin state, whether the
checkout is dirty, on a branch, or behind origin.

### `tins graph`

The tins in publish order — each one after everything it pins.

```
14. zstd-mojo
15. parquet-mojo  <- avro-mojo, brotli-mojo, hashes-mojo, lz4-mojo, snappy-mojo, thrift-mojo, zstd-mojo
17. iceberg-mojo  <- avro-mojo, hashes-mojo, ..., parquet-mojo
```

### `tins doctor`

Checks every repo against the registry and exits non-zero on an error.

| finding | level | means |
| --- | --- | --- |
| `version-mismatch` | error | the three places that carry the version disagree |
| `unpublished-pin` | error | a pin names a revision the registry never published |
| `outdated-pin` | warning | a pin names an older release than the newest |
| `stale-release` | error | the published version's revision is behind main, and `src/` changed |
| `unpublished` | warning | the local version is not on the registry yet |
| `shelf-tins-drift` | warning | `shelf.toml`'s `tins` list omits something the package pins |
| `unreleased-commits` | info | main is past the published revision, but only CI/docs moved |
| `behind`, `dirty`, `duplicate-checkout` | info | local checkout state; `-v` to show |

`unpublished-pin` and `outdated-pin` are deliberately different findings.
Pinning last month's release is a choice. Pinning a revision the registry
never saw is the defect that has broken installs twice: pixi resolves the
tin's own dependency at the published revision and your pin at another one,
gets two source records for a single package, and fails.

`stale-release` is the one that catches merged-but-unpublished work. A
version string stays valid while the tree it names falls behind main, so
nothing else notices that a merged fix reaches nobody. It fires only when
`src/` differs between the published revision and main — a README or CI
commit moves the sha without changing a byte anyone installs, and is
reported as `unreleased-commits` instead.

**Doctor reads `origin/main`, not your working tree.** It fetches first, so
the answer describes what a consumer would install rather than what happens
to be checked out. `--no-fetch` reads the working tree instead.

### `tins sweep`

Runs a command in a worktree of every selected repo and, wherever the tree
changed, commits, pushes and opens a PR.

```sh
tins sweep --org magmalake --branch repin-lint --title "Pin lint-mojo 0.1.2" --lock -- \
  sed -i '' 's/rev = "2a12807.*"/rev = "3fa66644756c1de7aece83e5a89375323b6e0ce7"/' pixi.toml
```

The command runs with the worktree as its working directory and
`TINS_REPO`, `TINS_ORG`, `TINS_TIN`, `TINS_VERSION` and `TINS_PATH` in the
environment. It is executed directly, not through a shell; for anything
with a pipe or a conditional, use `--script`. A repo where the command
fails, or changes nothing, is reported and skipped. `--lock` runs `pixi
lock` before committing; `--no-pr` pushes without opening one.

Only tracked files are committed, so build logs left in a worktree do not
end up in the PR.

### `tins release`

Opens a version-bump PR for every tin whose published revision is behind
main with `src/` changes — the other half of `doctor`'s `stale-release`
finding. The bump is the whole diff; the code is already on main and
already reviewed.

```sh
tins release --org magmalake          # print the plan
tins release --org magmalake --yes    # open the PRs
```

The PR body is generated: the published revision, main's revision, the
commit subjects between them, and the `src/` files that changed. Repos
whose version is *already* ahead of the registry are skipped — the bump has
happened and it is `publish`'s turn. `--force` includes repos where only CI
or docs moved, which normally owe no release.

The full release loop is three commands:

```sh
tins release --org magmalake --yes    # open the version-bump PRs
tins merge   --org magmalake --yes    # validate and merge them
tins publish --org magmalake --yes    # push the releases to the registry
```

### `tins merge`

Merges open pull requests that are **provably nothing but a version bump**,
and refuses everything else. It is `release`'s counterpart: that command
opens the PRs, and what is left over is reading each diff to confirm it is
still only a bump.

```sh
tins merge --org magmalake          # every open PR, with its verdict
tins merge --org magmalake --yes    # squash-merge the ones that passed
```

The value is not the click, it is not having to read the diffs — and that
review is exactly the kind that decays, because the fifth identical release
diff gets less attention than the first. So a PR is merged only if **all** of
these hold, and a failure names the rule rather than saying "invalid":

| rule | a PR fails it when |
| --- | --- |
| `foreign-file` | anything outside `pixi.toml` and `shelf.toml` changed — source, CI, README, or `pixi.lock` |
| `non-version-line` | a changed line is not a bare `version = "x.y.z"` at column zero |
| `malformed-patch`, `unreadable-patch` | the diff is in a shape the parser does not recognise, or GitHub rendered none |
| `unbalanced-change` | a version line was removed without one added in its place |
| `version-disagreement` | the diff names more than one old version, or more than one new one |
| `malformed-version` | either version is not a plain three-part version |
| `not-a-bump` | the new version is equal, backwards, or moves more than one part |
| `incomplete-bump` | a version line elsewhere in the merged tree still reads the old version |
| `already-published` | the new version is on the registry already |
| `checks-failing`, `checks-pending`, `no-checks` | CI is not green at the head revision |
| `not-mergeable`, `draft`, `wrong-base` | GitHub would refuse the merge, or this is not a release PR |

Two of those are worth spelling out.

**`pixi.lock` is not a version file.** The lock records resolved
dependencies; it does not carry the workspace's own version, so a bump needs
no relock and a lock in the diff means something else changed too. (If you
find a repo where the lock *does* carry it, that is worth saying out loud
rather than relaxing the rule.)

**`incomplete-bump` needs more than the diff.** The version lives in three
places — `pixi.toml` twice and `shelf.toml` once — and a PR that moves two of
them has a diff that looks perfect. So the version files are also read at the
head revision, and every version line in them must read the new version.
Anything less is `doctor`'s `version-mismatch` waiting to happen: `shelf
publish` reads one number and pixi reads another.

The one soft rule is `registry-behind`: if the *old* version is not what the
registry currently publishes, that is a warning with an explanation, not a
refusal — a repo can legitimately be several bumps ahead of the registry. The
new version already being published is a hard stop, because `tins publish`
skips a version the registry has and the merged code would never ship.

Merging is squash by default, matching how this org merges; `--merge-method`
takes `merge` or `rebase`. The merge passes `--match-head-commit`, so a push
landing between the validation and the merge aborts it rather than merging a
diff nobody checked. `--allow-no-checks` downgrades an empty check rollup to
a warning, for a repo with no CI — but note that an empty rollup is also what
a PR looks like in the seconds between the push and its workflows starting,
which is why it is a refusal by default.

Approval is not part of this. GitHub refuses `gh pr review --approve` on your
own pull request and these PRs are opened under your account, so merging is
the whole operation.

The validator fails closed on purpose: anything it cannot parse confidently
is a refusal. One that accepted a smuggled source edit would be worse than no
command at all, because it would turn a careful habit into a rubber stamp.
That is the part with the most tests.

### `tins publish`

Publishes every selected tin whose `shelf.toml` version is not yet on the
registry, in dependency order, from a throwaway worktree at the tip of
`origin/main`.

```sh
tins publish --org magmalake        # print the plan
tins publish --org magmalake --yes  # do it
```

It refuses to publish a tin whose package pins name unpublished revisions
(`--force` overrides), and it stops at the first failure rather than
letting a later tin pin a revision that never made it to the registry.

### `tins repin`

For every repo with a stale pin: move the revision, bump the version if a
*package* dependency moved, lock, commit, push, open a PR.

```sh
tins repin --org magmalake --dry-run
tins repin --org magmalake --package-only
```

The version bump is conditional on purpose. A `[package.*-dependencies]`
change is what consumers resolve, so it needs a release of its own; a pin
under `[dependencies]` or a feature only affects this repo's own
environments and ships without one.

## What it knows that you would otherwise have to remember

- **A package pin must name a revision the registry published.** Local HEAD
  drifts the moment a README commit lands.
- **The version lives in three files** — `pixi.toml` twice and
  `shelf.toml` — and `shelf publish` reads the third one.
- **Publish bottom-up.** The order comes from the package graph.
- **Locks are written by the pinned pixi**, not whatever is on `PATH`: a
  newer pixi refuses a lock an older one wrote.
- **`shelf publish` needs HEAD on a remote-tracking branch**, so publishing
  fetches over https and points `refs/remotes/origin/main` at that sha.
- **Remotes are addressed by https**, always. The `origin` ssh URLs are not
  reachable from this machine.
- **Commits carry one trailer, `Co-Authored-By`.** No session URL: it
  resolves for one person and these repos are public.

## Safety

Nothing that mutates ever runs in a shared checkout. Every write happens in
a worktree beside it (`../<repo>.<branch>`), so a checkout you were in the
middle of something in is never checked out, reset, stashed or pulled from
under you. `publish` uses a throwaway worktree it removes afterwards.

`sweep` and `repin` take `--dry-run`; `publish`, `release` and `merge` print
a plan and do nothing until `--yes`. `merge` never touches a checkout at
all — it reads GitHub and the registry, and the only thing it writes is the
merge itself.

## Development

```sh
pixi run test    # unit tests for the manifest rewrites
pixi run check   # tests plus a compile pass
```

The manifest edits are line-surgical rather than a round-trip through a
TOML writer, because these manifests carry comments explaining why each pin
is what it is and a round-trip drops them. That, and the `merge` validator in
`versionpatch.py`, are the parts with tests — the validator's cases are
fixture patches taken from real pull requests, so they do not depend on
anything being open today.

CI runs `pixi run check` on ubuntu and macos for changes under `tins/`.

Licensed under the MIT license of the repository that contains it.
