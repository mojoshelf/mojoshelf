"""The commands themselves."""

from __future__ import annotations

import os
import shutil
from dataclasses import dataclass, field
from pathlib import Path

from . import gitutil, manifest, versionpatch
from .config import Config
from .registry import Registry
from .workspace import GitDep, Repo, discover, select, topo_order

ERROR, WARN, INFO = "error", "warn", "info"


@dataclass
class Finding:
    repo: str
    level: str
    code: str
    message: str


# ---------------------------------------------------------------- helpers


def _worktree_on_branch(repo: Repo, branch: str, sha: str) -> Path:
    """A worktree of `repo` on `branch`, created from `sha` if it is new.

    The shared checkout is never touched: it holds whatever the user was
    doing, and a stray checkout or reset there is not recoverable from
    here.
    """
    wt = repo.worktree_path(branch)
    if wt.exists():
        return wt
    exists = gitutil.run(
        ["git", "rev-parse", "--verify", "--quiet", f"refs/heads/{branch}"],
        cwd=repo.path,
        check=False,
    )[2] == 0
    if exists:
        gitutil.git(repo.path, "worktree", "add", "--quiet", str(wt), branch)
    else:
        gitutil.add_worktree(repo.path, wt, sha, branch=branch)
    return wt


def _lock(config: Config, path: Path) -> None:
    gitutil.run(config.pixi_cmd("lock"), cwd=path)


@dataclass
class PinState:
    """How a single pinned dependency compares to the registry."""

    dep: GitDep
    pinned_version: str | None  # the release this rev is, if it is one
    latest_version: str | None
    latest_sha: str | None

    @property
    def unpublished(self) -> bool:
        """The pinned rev is not any published release.

        This is the one that breaks consumers: pixi resolves the tin's own
        dependency at the published rev and this pin at another, giving two
        source records for one package and a failed install. Pinning local
        HEAD instead of the published rev is how it happens.
        """
        return self.pinned_version is None

    @property
    def outdated(self) -> bool:
        """A published rev, but not the newest one. Legitimate, sometimes."""
        return not self.unpublished and self.dep.rev != self.latest_sha


def _pin_states(repo: Repo, registry: Registry) -> list[PinState]:
    states = []
    for dep in repo.deps:
        releases = registry.releases(dep.pkg)
        if not releases:
            continue  # not on the registry; nothing to compare against
        pinned = next((r for r in releases if r.sha.startswith(dep.rev[:12])), None)
        latest = releases[0]
        states.append(
            PinState(dep, pinned.version if pinned else None, latest.version, latest.sha)
        )
    return states


def _stale_deps(repo: Repo, registry: Registry) -> list[PinState]:
    """Pins that `tins repin` would move: unpublished or behind."""
    return [s for s in _pin_states(repo, registry) if s.unpublished or s.outdated]


def _read_body(body_file: str | None, body: str | None) -> str:
    """The PR/commit body, from a file or the flag.

    A missing --body-file is a typo, and it surfaces before any repo is
    touched: finding out after eight of ten repos have branches pushed is
    the expensive way to learn it.
    """
    if not body_file:
        return body or ""
    p = Path(body_file).expanduser()
    if not p.is_file():
        raise SystemExit(f"no such body file: {p}")
    try:
        return p.read_text()
    except OSError as e:
        raise SystemExit(f"cannot read body file {p}: {e}") from e


def _changed_paths(repo: Repo, old: str, new: str) -> list[str]:
    """Files that differ between two revisions of `repo`.

    Returns [] when either revision is missing from the local object store —
    a shallow clone, or a published rev on a branch that was never fetched —
    so a repo we cannot inspect is reported as nothing rather than as a
    confident wrong answer.
    """
    out, _, code = gitutil.run(
        ["git", "diff", "--name-only", f"{old}..{new}"], cwd=repo.path, check=False
    )
    return out.splitlines() if code == 0 and out else []


def _print_findings(findings: list[Finding], verbose: bool = False) -> None:
    shown = [f for f in findings if verbose or f.level != INFO]
    by_repo: dict[str, list[Finding]] = {}
    for f in shown:
        by_repo.setdefault(f.repo, []).append(f)
    for repo, items in sorted(by_repo.items()):
        print(f"\n{repo}")
        for f in items:
            mark = {"error": "  ✗", "warn": "  !", "info": "  ·"}[f.level]
            print(f"{mark} {f.code}: {f.message}")


# ------------------------------------------------------------------ list


def cmd_list(args, config: Config) -> int:
    registry = Registry(config.registry)
    repos = select(discover(config, fetch=not args.no_fetch), args.org, args.repo, args.tins)
    rows = []
    for r in sorted(repos, key=lambda r: (r.org, r.name)):
        published = registry.latest(r.tin) if r.tin else None
        pub = published.version if published else "—"
        if published and r.version and r.version != published.version:
            pub += " (local ahead)"
        stale = len(_stale_deps(r, registry))
        state = []
        if r.dirty():
            state.append("dirty")
        if (b := r.branch()) != "main":
            state.append(b)
        if r.ref and r.ref != r.head():
            state.append("behind")
        rows.append(
            (
                r.slug,
                r.version or "—",
                pub,
                "—" if not r.deps else ("ok" if not stale else f"{stale} stale"),
                ",".join(state) or "clean",
            )
        )
    widths = [max(len(str(row[i])) for row in [("repo", "local", "published", "pins", "state")] + rows) for i in range(5)]
    header = ("repo", "local", "published", "pins", "state")
    for row in [header, tuple("-" * w for w in widths)] + rows:
        print("  ".join(str(c).ljust(w) for c, w in zip(row, widths)).rstrip())
    return 0


# ----------------------------------------------------------------- graph


def cmd_graph(args, config: Config) -> int:
    repos = select(discover(config), args.org, args.repo, tins_only=True)
    by_tin = {r.tin: r for r in repos if r.tin}
    for i, r in enumerate(topo_order(repos), 1):
        deps = sorted(
            {d.pkg for d in r.deps if d.is_package_dep and d.pkg in by_tin and by_tin[d.pkg] is not r}
        )
        suffix = f"  <- {', '.join(deps)}" if deps else ""
        print(f"{i:2}. {r.tin or r.name}{suffix}")
    return 0


# ---------------------------------------------------------------- doctor


def cmd_doctor(args, config: Config) -> int:
    registry = Registry(config.registry)
    repos = select(discover(config, fetch=not args.no_fetch), args.org, args.repo, args.tins)
    findings: list[Finding] = []

    for r in sorted(repos, key=lambda r: (r.org, r.name)):
        # The version is written in up to three files and they drift.
        versions = {k: v for k, v in r.version_files.items() if v is not None}
        if len(set(versions.values())) > 1:
            detail = ", ".join(f"{k}={v}" for k, v in versions.items())
            findings.append(Finding(r.slug, ERROR, "version-mismatch", detail))

        for s in _pin_states(r, registry):
            dep = s.dep
            if s.unpublished:
                findings.append(
                    Finding(
                        r.slug,
                        ERROR if dep.is_package_dep else WARN,
                        "unpublished-pin",
                        f"{dep.pkg} pinned at {dep.rev[:12]} in [{dep.table}], which is not a "
                        f"published release; latest is {s.latest_version} at {s.latest_sha[:12]}",
                    )
                )
            elif s.outdated:
                findings.append(
                    Finding(
                        r.slug,
                        WARN if dep.is_package_dep else INFO,
                        "outdated-pin",
                        f"{dep.pkg} pinned at {s.pinned_version} in [{dep.table}]; "
                        f"latest is {s.latest_version} ({s.latest_sha[:12]})",
                    )
                )

        for dep in r.deps:
            if not registry.known(dep.pkg):
                findings.append(
                    Finding(r.slug, INFO, "unregistered-dep", f"{dep.pkg} is not on the registry")
                )

        if r.publishable and r.version:
            if not registry.known(r.tin):
                findings.append(Finding(r.slug, INFO, "unregistered", f"{r.tin} is not on the registry"))
            elif not (rel := registry.published(r.tin, r.version)):
                latest = registry.latest(r.tin)
                findings.append(
                    Finding(
                        r.slug,
                        WARN,
                        "unpublished",
                        f"{r.version} is not on the registry (latest is {latest.version if latest else 'none'})",
                    )
                )
            elif r.ref and rel.sha != r.ref:
                # The version is published but main has moved on without a
                # bump, so two trees answer to one version and everyone
                # installing gets the older one. Nothing else here notices,
                # because the version string itself is perfectly fine.
                #
                # Whether that matters depends on what moved: a CI or README
                # commit changes the sha without changing a byte a consumer
                # installs, and calling that an error would cry wolf on every
                # repo. Only a change under src/ is a release that is owed.
                changed = _changed_paths(r, rel.sha, r.ref)
                packaged = [p for p in changed if p.startswith("src/")]
                if packaged:
                    findings.append(
                        Finding(
                            r.slug,
                            ERROR,
                            "stale-release",
                            f"{r.version} is published at {rel.sha[:12]} but main is at "
                            f"{r.ref[:12]} with {len(packaged)} changed file(s) under src/: "
                            f"the merged work reaches nobody until the version is bumped "
                            f"and published",
                        )
                    )
                elif changed:
                    findings.append(
                        Finding(
                            r.slug,
                            INFO,
                            "unreleased-commits",
                            f"main is {r.ref[:12]}, past {r.version}'s {rel.sha[:12]}, but "
                            f"nothing under src/ changed — no release owed",
                        )
                    )

        if r.publishable and (declared := (r.shelf or {}).get("tins")) is not None:
            actual = {d.pkg for d in r.deps if d.is_package_dep}
            if missing := actual - set(declared):
                findings.append(
                    Finding(
                        r.slug,
                        WARN,
                        "shelf-tins-drift",
                        f"pinned but not listed in shelf.toml tins: {', '.join(sorted(missing))}",
                    )
                )

        if r.dirty():
            findings.append(Finding(r.slug, INFO, "dirty", "checkout has uncommitted changes"))
        for other in r.other_checkouts:
            findings.append(
                Finding(
                    r.slug,
                    INFO,
                    "duplicate-checkout",
                    f"also cloned at {other}; using {r.path}",
                )
            )
        if r.ref and r.head() and r.ref != r.head():
            findings.append(
                Finding(
                    r.slug,
                    INFO,
                    "behind",
                    f"checkout is at {r.head()[:8]}, origin/main is at {r.ref[:8]} (checked against origin/main)",
                )
            )

    errors = sum(1 for f in findings if f.level == ERROR)
    warns = sum(1 for f in findings if f.level == WARN)
    source = "working tree" if args.no_fetch else "origin/main"
    if not errors and not warns and not args.verbose:
        print(f"{len(repos)} repos checked against {source}, nothing to report")
        return 0
    _print_findings(findings, args.verbose)
    print(f"\n{len(repos)} repos checked against {source}: {errors} errors, {warns} warnings")
    return 1 if errors else 0


# ----------------------------------------------------------------- sweep


def cmd_sweep(args, config: Config) -> int:
    repos = select(discover(config, fetch=True), args.org, args.repo, args.tins)
    if args.script:
        script = Path(args.script).expanduser().resolve()
        if not script.is_file():
            raise SystemExit(f"no such script: {script}")
        argv = [str(script)]
    elif args.command:
        argv = list(args.command)
    else:
        raise SystemExit("give a command after -- or pass --script")

    body = _read_body(args.body_file, args.body)
    results: list[tuple[str, str]] = []

    for r in sorted(repos, key=lambda r: (r.org, r.name)):
        print(f"\n=== {r.slug}")
        if args.dry_run:
            results.append((r.slug, f"would run in {r.worktree_path(args.branch)}"))
            continue
        sha = r.ref or gitutil.fetch_main(r.path, r.org, r.name)
        # A sweep is usually aimed wider than it lands — a pin bump touches
        # ten repos out of nineteen. Remember whether this worktree is ours
        # so the ones that change nothing can be cleaned up instead of left
        # scattered around the tree.
        created = not r.worktree_path(args.branch).exists()
        wt = _worktree_on_branch(r, args.branch, sha)
        env = os.environ | {
            "TINS_REPO": r.name,
            "TINS_ORG": r.org,
            "TINS_TIN": r.tin or "",
            "TINS_VERSION": r.version or "",
            "TINS_PATH": str(wt),
        }
        _, err, code = gitutil.run(argv, cwd=wt, check=False, env=env)
        if code != 0:
            print(f"  command failed [{code}] {err.splitlines()[-1] if err else ''}")
            results.append((r.slug, f"command failed [{code}]"))
            continue
        if not gitutil.is_dirty(wt):
            if created:
                gitutil.remove_worktree(r.path, wt)
                gitutil.run(
                    ["git", "branch", "-D", args.branch], cwd=r.path, check=False
                )
            print("  no change")
            results.append((r.slug, "no change"))
            continue
        if args.lock:
            print("  locking")
            _lock(config, wt)
        message = gitutil.commit_message(args.title, body, config.co_authored_by)
        commit = gitutil.commit(wt, message, config.author_name, config.author_email)
        gitutil.push(wt, r.org, r.name, args.branch)
        url = gitutil.create_pr(r.org, r.name, args.branch, args.title, body) if args.pr else ""
        print(f"  {commit[:8]} pushed{'  ' + url if url else ''}")
        results.append((r.slug, url or commit[:8]))

    print("\n--- summary")
    for slug, outcome in results:
        print(f"{slug}: {outcome}")
    return 0


# --------------------------------------------------------------- publish


def cmd_publish(args, config: Config) -> int:
    registry = Registry(config.registry)
    # Manifests come from origin/main: publishing the version in a stale
    # working tree would publish the wrong number, or nothing at all.
    repos = [r for r in select(discover(config, fetch=True), args.org, args.repo) if r.publishable]
    ordered = topo_order(repos)

    plan: list[tuple[Repo, str, str]] = []  # repo, version, sha of main
    for r in ordered:
        sha = r.ref or gitutil.fetch_main(r.path, r.org, r.name)
        version = r.version
        if not version:
            plan.append((r, "?", sha))
            continue
        if registry.published(r.tin, version):
            print(f"skip  {r.slug} {version} already published")
            continue
        # Only an unpublished pin is disqualifying. Pinning an older
        # release is a choice; pinning a rev the registry never saw is the
        # defect that makes a consumer's install fail.
        bad = [s for s in _pin_states(r, registry) if s.unpublished and s.dep.is_package_dep]
        if bad and not args.force:
            print(
                f"BLOCK {r.slug} {version}: package pins that are not published releases "
                f"({', '.join(s.dep.pkg for s in bad)}) — run `tins repin` first, or --force"
            )
            continue
        plan.append((r, version, sha))

    if not plan:
        print("nothing to publish")
        return 0
    print("\nwill publish, in order:")
    for r, version, sha in plan:
        print(f"  {r.tin} {version}  from {r.slug}@{sha[:8]}")
    if not args.yes:
        print("\nre-run with --yes to publish")
        return 0

    for r, version, sha in plan:
        wt = r.path.parent / f"{r.name}.publish"
        gitutil.remove_worktree(r.path, wt)
        gitutil.add_worktree(r.path, wt, sha)
        # shelf publish will not publish a HEAD it cannot see on a
        # remote-tracking branch, and origin is an ssh URL we cannot reach.
        gitutil.update_remote_ref(wt, sha)
        out, err, code = gitutil.run(["shelf", "publish"], cwd=wt, check=False)
        print(f"{r.tin} {version}: {out or err}")
        gitutil.remove_worktree(r.path, wt)
        if wt.exists():
            shutil.rmtree(wt, ignore_errors=True)
        if code != 0:
            print("stopping: a later tin would pin an unpublished rev")
            return 1
    return 0


# ----------------------------------------------------------------- repin


def cmd_repin(args, config: Config) -> int:
    registry = Registry(config.registry)
    repos = select(discover(config, fetch=True), args.org, args.repo, args.tins)
    results: list[tuple[str, str]] = []

    for r in topo_order(sorted(repos, key=lambda r: (r.org, r.name))):
        stale = _stale_deps(r, registry)
        if args.package_only:
            stale = [s for s in stale if s.dep.is_package_dep]
        if args.unpublished_only:
            stale = [s for s in stale if s.unpublished]
        if not stale:
            continue
        print(f"\n=== {r.slug}")
        for s in stale:
            was = s.pinned_version or "unpublished"
            print(
                f"  {s.dep.pkg} {s.dep.rev[:12]} ({was}) -> {s.latest_sha[:12]} "
                f"({s.latest_version}) [{s.dep.table}]"
            )
        if args.dry_run:
            results.append((r.slug, "dry run"))
            continue

        sha_main = r.ref or gitutil.fetch_main(r.path, r.org, r.name)
        wt = _worktree_on_branch(r, args.branch, sha_main)
        pixi_path = wt / "pixi.toml"
        text = pixi_path.read_text()
        for s in stale:
            text, n = manifest.set_dep_rev(text, s.dep.pkg, s.latest_sha)
            if n == 0:
                print(f"  ! could not rewrite {s.dep.pkg} — leaving it")
        pixi_path.write_text(text)

        # A package-dep move changes what consumers resolve, so it needs a
        # release of its own. A workspace-only move does not.
        bumped = ""
        if any(s.dep.is_package_dep for s in stale) and r.version:
            new_version = manifest.bump(r.version, args.bump)
            text, n = manifest.set_version(pixi_path.read_text(), r.version, new_version)
            pixi_path.write_text(text)
            if (shelf_path := wt / "shelf.toml").is_file():
                shelf_text, m = manifest.set_version(shelf_path.read_text(), r.version, new_version)
                shelf_path.write_text(shelf_text)
                n += m
            print(f"  version {r.version} -> {new_version} ({n} files)")
            bumped = f"; {r.version} -> {new_version}"

        _lock(config, wt)
        # One entry per package, not one per table it is pinned in.
        moved = ", ".join(
            f"{pkg} -> {version}"
            for pkg, version in dict.fromkeys((s.dep.pkg, s.latest_version) for s in stale)
        )
        title = args.title or f"Re-pin {', '.join(sorted({s.dep.pkg for s in stale}))}"
        body = (
            f"Pins moved to the newest published release{bumped}.\n\n"
            + "\n".join(
                f"- `{s.dep.pkg}` `{s.dep.rev[:12]}` "
                f"({s.pinned_version or 'not a published release'}) → "
                f"`{s.latest_sha[:12]}` ({s.latest_version}) in `[{s.dep.table}]`"
                for s in stale
            )
            + "\n\nA package dependency must name a revision the registry actually published:"
            " when a pin names something else, pixi resolves two source records for the same"
            " package and the install fails.\n"
        )
        message = gitutil.commit_message(title, body, config.co_authored_by)
        commit = gitutil.commit(wt, message, config.author_name, config.author_email)
        gitutil.push(wt, r.org, r.name, args.branch)
        url = gitutil.create_pr(r.org, r.name, args.branch, title, body) if args.pr else ""
        print(f"  {commit[:8]} pushed{'  ' + url if url else ''}")
        results.append((r.slug, url or commit[:8]))
        print(f"  ({moved})")

    if not results:
        print("every pin is current")
    else:
        print("\n--- summary")
        for slug, outcome in results:
            print(f"{slug}: {outcome}")
    return 0


# --------------------------------------------------------------- release


def _commits_between(repo: Repo, old: str, new: str) -> list[str]:
    """Commit subjects in `old..new`, newest first."""
    out, _, code = gitutil.run(
        ["git", "log", "--format=%s", f"{old}..{new}"], cwd=repo.path, check=False
    )
    return out.splitlines() if code == 0 and out else []


def cmd_release(args, config: Config) -> int:
    """Open a version-bump PR for every tin whose published rev is behind main.

    This is the other half of `doctor`'s stale-release finding: that check
    says a release is owed, and this opens it. The bump is the whole change —
    the code is already on main and already reviewed.
    """
    registry = Registry(config.registry)
    repos = select(discover(config, fetch=True), args.org, args.repo)

    plan: list[tuple[Repo, str, str, list[str], list[str]]] = []
    for r in topo_order(repos):
        if not r.publishable or not r.version or not r.ref:
            continue
        rel = registry.published(r.tin, r.version)
        if not rel:
            # The version is already ahead of the registry — the bump has
            # happened and it is `tins publish`'s turn, not ours.
            continue
        if rel.sha == r.ref:
            continue
        changed = _changed_paths(r, rel.sha, r.ref)
        packaged = [p for p in changed if p.startswith("src/")]
        if not packaged and not args.force:
            continue
        plan.append(
            (r, r.version, manifest.bump(r.version, args.bump), packaged, changed)
        )

    if not plan:
        print("no release owed")
        return 0

    print("will open version-bump PRs for:")
    for r, old, new, packaged, changed in plan:
        print(f"  {r.tin}  {old} -> {new}   ({len(packaged)} src file(s) of {len(changed)})")
    if not args.yes:
        print("\nre-run with --yes to open them")
        return 0

    results: list[tuple[str, str]] = []
    for r, old, new, packaged, changed in plan:
        print(f"\n=== {r.slug}")
        branch = args.branch or f"release-{new}"
        wt = _worktree_on_branch(r, branch, r.ref)

        touched = 0
        for name in ("pixi.toml", "shelf.toml"):
            f = wt / name
            if not f.is_file():
                continue
            text, n = manifest.set_version(f.read_text(), old, new)
            if n:
                f.write_text(text)
                touched += n
        if touched == 0:
            print(f"  ! no version line matched {old} — skipping")
            results.append((r.slug, f"no version line matched {old}"))
            continue
        print(f"  {old} -> {new} ({touched} line(s))")

        subjects = _commits_between(r, registry.published(r.tin, old).sha, r.ref)
        body = (
            f"Version bump only. `{old}` is published at "
            f"`{registry.published(r.tin, old).sha[:12]}` and main is at `{r.ref[:12]}` "
            f"with {len(packaged)} changed file(s) under `src/`, so nobody installing "
            f"the tin gets any of them.\n\n"
            + ("**Changes since the published revision**\n\n"
               + "\n".join(f"- {s}" for s in subjects[:20])
               + ("\n- …\n" if len(subjects) > 20 else "\n")
               if subjects else "")
            + "\n**Files under `src/`**\n\n"
            + "\n".join(f"- `{p}`" for p in packaged[:20])
            + ("\n- …\n" if len(packaged) > 20 else "\n")
        )
        title = args.title or f"Release {new}"
        message = gitutil.commit_message(title, body, config.co_authored_by)
        commit = gitutil.commit(wt, message, config.author_name, config.author_email)
        gitutil.push(wt, r.org, r.name, branch)
        url = gitutil.create_pr(r.org, r.name, branch, title, body) if args.pr else ""
        print(f"  {commit[:8]} pushed{'  ' + url if url else ''}")
        results.append((r.slug, url or commit[:8]))

    print("\n--- summary")
    for slug, outcome in results:
        print(f"{slug}: {outcome}")
    print("\n`tins merge --yes`, then `tins publish --yes`")
    return 0


# ----------------------------------------------------------------- merge


# A check that finished in one of these did not fail. SKIPPED and NEUTRAL
# are how a workflow says "not applicable here", which is what GitHub's own
# required-checks rule treats them as; anything else — failure, cancelled,
# timed out, action required — is a refusal.
GOOD_CONCLUSIONS = {"SUCCESS", "SKIPPED", "NEUTRAL"}

# How many failures of one rule to print before summarising the rest.
PROBLEMS_PER_CODE = 3

# `mergeStateStatus` values that mean GitHub itself would refuse or would
# merge something other than what we read. CLEAN, HAS_HOOKS and UNSTABLE are
# the ones we let through — UNSTABLE only ever means a non-required check is
# unhappy, and the check rule below judges those on its own terms.
UNMERGEABLE_STATES = {
    "DIRTY": "the branch conflicts with main",
    "BLOCKED": (
        "branch protection blocks the merge — a required review, most likely, and GitHub "
        "will not let you approve your own pull request"
    ),
    "BEHIND": "the branch is behind main and this repo requires branches to be up to date",
    "UNKNOWN": "GitHub has not finished computing the merge state; try again in a moment",
}


@dataclass
class Verdict:
    """One open pull request and everything `tins merge` concluded about it."""

    org: str
    name: str
    number: int
    title: str
    url: str
    head: str
    notes: list[Finding] = field(default_factory=list)

    @property
    def key(self) -> str:
        return f"{self.org}/{self.name}#{self.number}"

    @property
    def failures(self) -> list[Finding]:
        return [f for f in self.notes if f.level == ERROR]

    @property
    def ok(self) -> bool:
        return not self.failures


def _check_state(rollup: list[dict] | None) -> tuple[str, str]:
    """A PR's CI as (state, description), where state is green/pending/failing/none.

    Two shapes arrive in one list: GitHub Actions jobs (`CheckRun`, with a
    status and a conclusion) and commit statuses posted by other services
    (`StatusContext`, with a single state). Anything that is neither is
    counted as failing rather than ignored.
    """
    if not rollup:
        return "none", "no checks reported for the head revision"
    pending, failing, good = [], [], 0
    for c in rollup:
        label = c.get("name") or c.get("context") or "?"
        if c.get("__typename") == "CheckRun" or "conclusion" in c:
            status = (c.get("status") or "").upper()
            conclusion = (c.get("conclusion") or "").upper()
            if status != "COMPLETED":
                pending.append(f"{label} ({status.lower() or 'no status'})")
            elif conclusion in GOOD_CONCLUSIONS:
                good += 1
            else:
                failing.append(f"{label} ({conclusion.lower() or 'no conclusion'})")
        elif "state" in c:
            state = (c.get("state") or "").upper()
            if state in ("PENDING", "EXPECTED"):
                pending.append(f"{label} (pending)")
            elif state == "SUCCESS":
                good += 1
            else:
                failing.append(f"{label} ({state.lower() or 'no state'})")
        else:
            failing.append(f"{label} (unrecognised check)")
    total = len(rollup)
    if failing:
        return "failing", f"{len(failing)} of {total} failing: {', '.join(failing[:6])}"
    if pending:
        return "pending", f"{len(pending)} of {total} still running: {', '.join(pending[:6])}"
    return "green", f"{good} of {total} passed"


def _judge_pr(repo: Repo, pr: dict, registry: Registry, args) -> Verdict:
    """Every rule, applied to one pull request.

    The order is the order a person would read them in: what changed, what
    the change says, whether the registry agrees, and only then whether
    GitHub would let the merge happen. A failure names its rule, because
    "invalid" does not tell you which line to go and look at.
    """
    v = Verdict(
        org=repo.org,
        name=repo.name,
        number=pr["number"],
        title=pr.get("title", ""),
        url=pr.get("url", ""),
        head=pr.get("headRefOid", ""),
    )

    def note(level: str, code: str, message: str) -> None:
        v.notes.append(Finding(v.key, level, code, message))

    if (base := pr.get("baseRefName")) != "main":
        note(ERROR, "wrong-base", f"targets {base}, not main; this is not a release PR")

    files = gitutil.pr_files(repo.org, repo.name, v.number)
    if files is None:
        note(ERROR, "unreadable-diff", "could not read the changed files from GitHub")
        return v
    bump = versionpatch.check_changed_files(
        [
            versionpatch.ChangedFile(f.get("filename", "?"), f.get("status", "?"), f.get("patch"))
            for f in files
        ]
    )
    # One repin PR moves the same six pins in three tables, which is
    # thirty-odd identical refusals. Three of each is enough to see what the
    # rule caught; the count says how much more there was.
    seen: dict[str, int] = {}
    for p in bump.problems:
        seen[p.code] = n = seen.get(p.code, 0) + 1
        if n <= PROBLEMS_PER_CODE:
            note(ERROR, p.code, p.message)
    for code, n in seen.items():
        if n > PROBLEMS_PER_CODE:
            note(ERROR, code, f"… and {n - PROBLEMS_PER_CODE} more like that")
    if not bump.ok:
        return v
    note(INFO, "version-bump", bump.summary())

    # The diff cannot show a version line the PR failed to touch, so read
    # the version files as they will be once merged.
    texts = {
        name: gitutil.file_at_ref(repo.org, repo.name, name, v.head)
        for name in versionpatch.VERSION_FILES
    }
    if texts["pixi.toml"] is None:
        note(ERROR, "unreadable-head", f"cannot read pixi.toml at {v.head[:12]}")
        return v
    for p in versionpatch.check_head_versions(texts, bump.new):
        note(ERROR, p.code, p.message)

    # Registry coherence. A new version that is already published is a hard
    # stop — merging it strands the release, because `tins publish` skips a
    # version the registry already has and the code never ships. An old
    # version that is not the published one is only a warning: a repo can
    # legitimately be several bumps ahead of the registry.
    if repo.tin and registry.known(repo.tin):
        if registry.published(repo.tin, bump.new):
            note(
                ERROR,
                "already-published",
                f"{repo.tin} {bump.new} is already on the registry; merging this would "
                f"produce a version `tins publish` skips, and the code would never ship",
            )
        latest = registry.latest(repo.tin)
        if latest and latest.version != bump.old:
            note(
                WARN,
                "registry-behind",
                f"the registry publishes {repo.tin} {latest.version}, not {bump.old}; the "
                f"repo may simply be bumps ahead of it — worth a look, not a refusal",
            )

    state, detail = _check_state(pr.get("statusCheckRollup"))
    if state == "green":
        note(INFO, "checks", detail)
    elif state == "none":
        # An empty rollup is also what a PR looks like in the seconds
        # between the push and the workflows starting, and merging then
        # merges an untested tree with a green-looking summary.
        note(
            WARN if args.allow_no_checks else ERROR,
            "no-checks",
            f"{detail}; either this repo has no CI or the workflows have not started yet"
            + (" (allowed by --allow-no-checks)" if args.allow_no_checks else ""),
        )
    else:
        note(ERROR, f"checks-{state}", detail)

    if pr.get("isDraft"):
        note(ERROR, "draft", "the pull request is a draft; a draft is never merged")
    mergeable = pr.get("mergeable")
    blocked = UNMERGEABLE_STATES.get(pr.get("mergeStateStatus") or "UNKNOWN")
    if mergeable != "MERGEABLE":
        note(
            ERROR,
            "not-mergeable",
            f"GitHub reports mergeable={mergeable}"
            + (
                "; it has not finished computing this yet, so try again in a moment"
                if mergeable == "UNKNOWN"
                else ""
            ),
        )
    elif blocked:
        note(ERROR, "not-mergeable", blocked)
    else:
        note(INFO, "mergeable", f"mergeable, merge state {pr.get('mergeStateStatus')}")
    return v


def cmd_merge(args, config: Config) -> int:
    """Merge open pull requests that are provably nothing but a version bump.

    The counterpart to `release`: that command opens these PRs, and the work
    left over is reading each diff to confirm it is still only a bump.
    Doing that by hand is exactly the review that decays — the fifth
    identical diff gets less attention than the first — so the rules live in
    `versionpatch` and this prints what each one concluded.

    Approval is not part of the loop: GitHub refuses `gh pr review
    --approve` on your own pull request and these are opened under the
    owner's account, so merging is the whole operation.

    Discovery does not fetch. Every fact that decides a verdict comes from
    GitHub or the registry; the local checkouts are here only to say which
    repos exist and which tin each one publishes.
    """
    registry = Registry(config.registry)
    repos = select(discover(config), args.org, args.repo, args.tins)

    verdicts: list[Verdict] = []
    unreadable: list[str] = []
    for r in sorted(repos, key=lambda r: (r.org, r.name)):
        prs = gitutil.open_prs(r.org, r.name)
        if prs is None:
            unreadable.append(r.slug)
            continue
        for pr in prs:
            verdicts.append(_judge_pr(r, pr, registry, args))

    for slug in unreadable:
        print(f"! {slug}: could not list its pull requests; it is not covered by this report")
    if not verdicts:
        print("no open pull requests")
        return 1 if unreadable else 0

    marks = {ERROR: "✗", WARN: "!", INFO: "✓"}
    for v in verdicts:
        print(f"\n{v.key}  {v.title}")
        if v.url:
            print(f"  {v.url}")
        for f in v.notes:
            print(f"  {marks[f.level]} {f.code}: {f.message}")
        print(f"  => {'MERGE' if v.ok else 'REFUSE'}")

    passing = [v for v in verdicts if v.ok]
    print(
        f"\n--- {len(verdicts)} open pull request(s): "
        f"{len(passing)} pure version bump(s), {len(verdicts) - len(passing)} refused"
        + (f"; {len(unreadable)} repo(s) unread" if unreadable else "")
    )
    if not passing:
        return 1 if unreadable else 0
    if not args.yes:
        print(f"re-run with --yes to {args.merge_method}-merge the {len(passing)} that passed")
        return 1 if unreadable else 0

    failed = 0
    for v in passing:
        out, code = gitutil.merge_pr(
            v.org, v.name, v.number, args.merge_method, head_sha=v.head or None
        )
        failed += code != 0
        print(f"{v.key}: {'merged' if code == 0 else 'FAILED'} {out.splitlines()[-1] if out else ''}")
    if not failed:
        print("\nnow `tins publish --yes`")
    return 1 if failed or unreadable else 0
