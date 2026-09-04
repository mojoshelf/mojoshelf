"""Discovering the checkouts and the dependency graph between them."""

from __future__ import annotations

import tomllib
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

from . import gitutil
from .config import Config


@dataclass(frozen=True)
class GitDep:
    """A dependency pinned at a git revision."""

    pkg: str
    url: str
    rev: str
    table: str  # e.g. "package.host-dependencies", "feature.bench.dependencies"

    @property
    def is_package_dep(self) -> bool:
        """Package deps propagate to consumers; workspace deps do not.

        A stale rev under `[package.*-dependencies]` reaches everyone who
        installs the tin, so fixing one needs a new release. A stale rev
        under `[dependencies]` or a feature only affects this repo's own
        environments, and can ship without a version bump.
        """
        return self.table.startswith("package.")


@dataclass
class Repo:
    path: Path
    org: str
    name: str
    pixi: dict
    pixi_text: str
    shelf: dict | None = None
    deps: list[GitDep] = field(default_factory=list)
    ref: str | None = None  # the sha these manifests were read from, if not the worktree
    other_checkouts: list[Path] = field(default_factory=list)

    # --- identity -------------------------------------------------------

    @property
    def slug(self) -> str:
        return f"{self.org}/{self.name}"

    @property
    def is_package(self) -> bool:
        return "package" in self.pixi

    @property
    def publishable(self) -> bool:
        """Has a shelf.toml, so `shelf publish` has something to publish."""
        return self.shelf is not None

    @property
    def tin(self) -> str | None:
        if self.shelf:
            return self.shelf.get("name")
        return self.pixi.get("package", {}).get("name")

    # --- versions -------------------------------------------------------

    @property
    def workspace_version(self) -> str | None:
        return self.pixi.get("workspace", {}).get("version")

    @property
    def package_version(self) -> str | None:
        return self.pixi.get("package", {}).get("version")

    @property
    def shelf_version(self) -> str | None:
        return (self.shelf or {}).get("version")

    @property
    def version(self) -> str | None:
        return self.shelf_version or self.package_version or self.workspace_version

    @property
    def version_files(self) -> dict[str, str | None]:
        """Every place the version is written, for the consistency check."""
        return {
            "pixi.toml [workspace]": self.workspace_version,
            "pixi.toml [package]": self.package_version,
            "shelf.toml": self.shelf_version,
        }

    # --- git ------------------------------------------------------------

    def head(self) -> str:
        return gitutil.head(self.path)

    def branch(self) -> str:
        return gitutil.current_branch(self.path)

    def dirty(self) -> bool:
        return gitutil.is_dirty(self.path)

    def worktree_path(self, branch: str) -> Path:
        return self.path.parent / f"{self.name}.{branch}"


def _collect_deps(pixi: dict) -> list[GitDep]:
    found: list[GitDep] = []

    def scan(table: dict, label: str) -> None:
        for pkg, spec in table.items():
            if isinstance(spec, dict) and "git" in spec and "rev" in spec:
                found.append(GitDep(pkg, spec["git"], spec["rev"], label))

    for name, table in pixi.get("package", {}).items():
        if name.endswith("dependencies") and isinstance(table, dict):
            scan(table, f"package.{name}")
    if isinstance(pixi.get("dependencies"), dict):
        scan(pixi["dependencies"], "dependencies")
    for feature, body in (pixi.get("feature") or {}).items():
        if isinstance(body, dict) and isinstance(body.get("dependencies"), dict):
            scan(body["dependencies"], f"feature.{feature}.dependencies")
    return found


def _read(path: Path, filename: str, ref: str | None) -> str | None:
    """A file's content, either from the working tree or from a git ref."""
    if ref is None:
        f = path / filename
        try:
            return f.read_text() if f.is_file() else None
        except OSError:
            return None
    out, _, code = gitutil.run(["git", "show", f"{ref}:{filename}"], cwd=path, check=False)
    return out if code == 0 else None


def load_repo(path: Path, ref: str | None = None) -> Repo | None:
    """Load a checkout's manifests, from the working tree or from `ref`.

    Reading from origin/main rather than the working tree is what makes
    the answers match what a consumer installs: a local checkout is
    routinely behind, and a doctor that reads it reports pins that were
    fixed days ago.
    """
    if not (path / "pixi.toml").is_file() or not gitutil.is_clone(path):
        return None
    remote = gitutil.remote_of(path)
    if not remote:
        return None
    pixi_text = _read(path, "pixi.toml", ref)
    if pixi_text is None:
        return None
    try:
        pixi = tomllib.loads(pixi_text)
    except tomllib.TOMLDecodeError:
        return None
    shelf = None
    if shelf_text := _read(path, "shelf.toml", ref):
        try:
            shelf = tomllib.loads(shelf_text)
        except tomllib.TOMLDecodeError:
            shelf = None
    org, name = remote
    return Repo(
        path=path,
        org=org,
        name=name,
        pixi=pixi,
        pixi_text=pixi_text,
        shelf=shelf,
        deps=_collect_deps(pixi),
    )


def discover(config: Config, fetch: bool = False) -> list[Repo]:
    """Every clone under the configured roots, keyed by its git remote.

    The org comes from the remote rather than the directory layout, because
    the checkouts are not grouped by org on disk. With `fetch`, each
    repo's manifests are read from the tip of origin/main instead of the
    working tree.
    """
    candidates: list[Path] = []
    seen: set[Path] = set()
    for root in config.roots:
        if not root.is_dir():
            continue
        for child in sorted(root.iterdir()):
            resolved = child.resolve()
            if not child.is_dir() or child.name.startswith(".") or resolved in seen:
                continue
            seen.add(resolved)
            candidates.append(child)

    def one(path: Path) -> Repo | None:
        ref = None
        if fetch:
            remote = gitutil.remote_of(path)
            if not remote:
                return None
            try:
                ref = gitutil.fetch_main(path, *remote)
            except gitutil.CommandError:
                ref = None  # unreachable remote: fall back to the working tree
        repo = load_repo(path, ref)
        if repo:
            repo.ref = ref
        return repo

    with ThreadPoolExecutor(max_workers=8) as pool:
        loaded = list(pool.map(one, candidates))

    kept: dict[str, Repo] = {}
    for r in loaded:
        if not r or r.slug in config.ignore or r.name in config.ignore:
            continue
        if (prev := kept.get(r.slug)) is None:
            kept[r.slug] = r
            continue
        # The same remote is cloned twice (an abandoned WIP copy, say).
        # Keep the checkout whose directory is named after the repo, so a
        # sweep does not run twice against one remote.
        primary, secondary = (r, prev) if r.path.name == r.name else (prev, r)
        primary.other_checkouts.append(secondary.path)
        primary.other_checkouts.extend(secondary.other_checkouts)
        kept[r.slug] = primary
    return list(kept.values())


def select(
    repos: list[Repo],
    org: str | None = None,
    names: list[str] | None = None,
    tins_only: bool = False,
) -> list[Repo]:
    out = repos
    if org:
        out = [r for r in out if r.org == org]
    if tins_only:
        out = [r for r in out if r.publishable]
    if names:
        wanted = set(names)
        out = [r for r in out if wanted & {r.name, r.tin or "", r.slug}]
        missing = wanted - {n for r in out for n in (r.name, r.tin or "", r.slug)}
        if missing:
            raise SystemExit(f"no such repo: {', '.join(sorted(missing))}")
    return out


def topo_order(repos: list[Repo]) -> list[Repo]:
    """Repos in publish order: a tin comes after everything it pins.

    Only package dependencies count — those are what a consumer resolves
    when it installs the tin.
    """
    by_tin = {r.tin: r for r in repos if r.tin}
    incoming = {
        r.slug: {
            d.pkg for d in r.deps if d.is_package_dep and d.pkg in by_tin and by_tin[d.pkg] is not r
        }
        for r in repos
    }
    remaining = {r.slug: r for r in repos}
    ordered: list[Repo] = []
    while remaining:
        ready = sorted(
            (
                r
                for r in remaining.values()
                if not (incoming[r.slug] & {x.tin for x in remaining.values() if x.tin})
            ),
            key=lambda r: (r.org, r.name),
        )
        if not ready:  # a cycle: fall back to a stable order rather than hanging
            ready = sorted(remaining.values(), key=lambda r: (r.org, r.name))
        for r in ready:
            ordered.append(r)
            del remaining[r.slug]
    return ordered
