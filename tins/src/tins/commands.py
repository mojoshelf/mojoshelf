"""The commands themselves."""

from __future__ import annotations

import os
import shutil
from dataclasses import dataclass
from pathlib import Path

from . import gitutil, manifest
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
            elif not registry.published(r.tin, r.version):
                latest = registry.latest(r.tin)
                findings.append(
                    Finding(
                        r.slug,
                        WARN,
                        "unpublished",
                        f"{r.version} is not on the registry (latest is {latest.version if latest else 'none'})",
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

    body = Path(args.body_file).read_text() if args.body_file else (args.body or "")
    results: list[tuple[str, str]] = []

    for r in sorted(repos, key=lambda r: (r.org, r.name)):
        print(f"\n=== {r.slug}")
        if args.dry_run:
            results.append((r.slug, f"would run in {r.worktree_path(args.branch)}"))
            continue
        sha = r.ref or gitutil.fetch_main(r.path, r.org, r.name)
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
