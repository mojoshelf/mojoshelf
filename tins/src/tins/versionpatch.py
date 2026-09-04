"""Deciding whether a pull request's diff is nothing but a version bump.

`tins release` opens one PR per tin that owes a release, and the diff is
always the same three lines. Reading each one to confirm that is what it
still is, is the tedious part — and the part a person does badly, because
after the fourth identical diff nobody is really reading the fifth.

So this module reads them instead, and the only interesting property is
that it is **wrong in the safe direction**. Every function here rejects
anything it cannot account for: an unfamiliar diff line, a patch GitHub
declined to render, a hunk header in a shape it does not recognise. A
validator that lets one smuggled source edit through is worse than no
validator, because it converts a careful review into a rubber stamp.

Nothing here talks to the network. The inputs are the changed-file list
from `pulls/<n>/files` and the text of the version files at the head
revision, which makes every rule testable against a fixture patch.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field

# The only files a version bump may touch. `pixi.lock` is deliberately not
# here: the lock records resolved dependencies, not the workspace's own
# version, so a bump needs no relock and a lock in the diff means something
# else changed too.
VERSION_FILES = ("pixi.toml", "shelf.toml")

# A version line the manifests actually carry, at column zero. Indentation
# would mean the line sits inside an inline table or a nested table — a
# dependency's version, not the package's — and `manifest.set_version`
# anchors the same way, so the two agree on what a version line is.
VERSION_LINE_RE = re.compile(r'^version[ \t]*=[ \t]*"(?P<version>[^"\n]*)"[ \t]*\r?$', re.M)

# Strict three-part semver: no pre-release, no build metadata, no leading
# zeros. `manifest.bump` can only produce this shape, so anything else in a
# release PR was written by hand and deserves a look.
SEMVER_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")

HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@")

NO_NEWLINE = "\\ No newline at end of file"


@dataclass(frozen=True)
class ChangedFile:
    """One entry of GitHub's `pulls/<n>/files`.

    `patch` is None when GitHub declined to render a diff — a binary file,
    or one too large. That is a rejection, not a shrug: an unrendered patch
    is precisely the place to hide something.
    """

    filename: str
    status: str
    patch: str | None


@dataclass(frozen=True)
class Problem:
    code: str
    message: str


@dataclass
class BumpVerdict:
    """What a diff turned out to be."""

    old: str | None = None
    new: str | None = None
    kind: str | None = None  # "major", "minor" or "patch"
    lines: dict[str, int] = field(default_factory=dict)  # file -> version lines changed
    problems: list[Problem] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return not self.problems and self.old is not None and self.new is not None

    def summary(self) -> str:
        files = ", ".join(f"{name} ({n} line(s))" for name, n in sorted(self.lines.items()))
        return f"{self.old} -> {self.new} ({self.kind}) in {files}"


def parse_semver(version: str) -> tuple[int, int, int] | None:
    m = SEMVER_RE.match(version.strip())
    return (int(m.group(1)), int(m.group(2)), int(m.group(3))) if m else None


def bump_kind(old: str, new: str) -> str | None:
    """Which part `old` -> `new` increments, or None if it is not a bump.

    A bump moves exactly one part up by one and zeroes everything below it.
    Equal versions, a version going backwards, and 1.2.3 -> 1.4.0 all return
    None: a release PR that does any of those was not written by
    `manifest.bump`, whatever else it may be.
    """
    a, b = parse_semver(old), parse_semver(new)
    if a is None or b is None:
        return None
    if b == (a[0], a[1], a[2] + 1):
        return "patch"
    if b == (a[0], a[1] + 1, 0):
        return "minor"
    if b == (a[0] + 1, 0, 0):
        return "major"
    return None


def scan_patch(filename: str, patch: str | None) -> tuple[list[str], list[str], list[Problem]]:
    """Every changed line of one file's patch, or a reason to refuse.

    Returns the versions on removed lines, the versions on added lines, and
    the problems found. A changed line that is not a bare version line is a
    problem — that is the smuggled edit this exists to catch — and so is any
    line whose role in the diff is not one of the four the format defines.
    """
    removed: list[str] = []
    added: list[str] = []
    problems: list[Problem] = []

    def bad(code: str, detail: str) -> None:
        problems.append(Problem(code, f"{filename}: {detail}"))

    if patch is None:
        bad(
            "unreadable-patch",
            "GitHub rendered no patch for this file (binary, or too large to diff); "
            "an unreadable change is a rejection, not an assumption",
        )
        return removed, added, problems

    in_hunk = False
    for raw in patch.split("\n"):
        line = raw.rstrip("\r")
        if line.startswith("@@"):
            if not HUNK_RE.match(line):
                bad("malformed-patch", f"unrecognised hunk header {line!r}")
            in_hunk = True
            continue
        if not in_hunk:
            bad("malformed-patch", f"content before the first hunk header: {line!r}")
            continue
        if line == "" or line.startswith(" ") or line == NO_NEWLINE:
            continue  # context, or the end-of-file marker
        if line[0] not in "-+":
            bad("malformed-patch", f"line is neither context, addition nor removal: {line!r}")
            continue
        m = VERSION_LINE_RE.match(line[1:])
        if not m:
            bad("non-version-line", f"changed line is not a version line: {line!r}")
            continue
        (removed if line[0] == "-" else added).append(m.group("version"))
    return removed, added, problems


def check_changed_files(files: list[ChangedFile]) -> BumpVerdict:
    """Judge a pull request's whole changed-file set.

    Four rules, in the order a reader would apply them: only version files
    changed; every changed line is a version line; one old version and one
    new version across all of them; and the new one is a forward bump of the
    old. Each failure names itself, because "invalid" tells you nothing
    about which line to go and look at.
    """
    verdict = BumpVerdict()
    if not files:
        verdict.problems.append(
            Problem("empty-diff", "the pull request changes no files; there is nothing to merge")
        )
        return verdict

    removed_all: list[str] = []
    added_all: list[str] = []
    for f in files:
        if f.filename not in VERSION_FILES:
            verdict.problems.append(
                Problem(
                    "foreign-file",
                    f"{f.filename} changed; a version bump may only touch "
                    f"{' and '.join(VERSION_FILES)}",
                )
            )
            continue
        if f.status != "modified":
            verdict.problems.append(
                Problem(
                    "foreign-file",
                    f"{f.filename} is {f.status}, not modified; a bump edits a line in place",
                )
            )
            continue
        removed, added, problems = scan_patch(f.filename, f.patch)
        verdict.problems.extend(problems)
        if problems:
            continue
        if len(removed) != len(added):
            verdict.problems.append(
                Problem(
                    "unbalanced-change",
                    f"{f.filename}: {len(removed)} line(s) removed but {len(added)} added; "
                    f"a bump replaces each version line in place",
                )
            )
            continue
        if not removed:
            verdict.problems.append(
                Problem("no-version-change", f"{f.filename}: modified, but no version line changed")
            )
            continue
        verdict.lines[f.filename] = len(removed)
        removed_all.extend(removed)
        added_all.extend(added)

    if verdict.problems:
        return verdict

    olds, news = set(removed_all), set(added_all)
    if len(olds) != 1 or len(news) != 1:
        verdict.problems.append(
            Problem(
                "version-disagreement",
                f"the diff does not describe one bump: it removes {sorted(olds)} "
                f"and adds {sorted(news)}",
            )
        )
        return verdict

    old, new = olds.pop(), news.pop()
    for label, version in (("old", old), ("new", new)):
        if parse_semver(version) is None:
            verdict.problems.append(
                Problem(
                    "malformed-version",
                    f"the {label} version {version!r} is not a plain three-part version",
                )
            )
    if verdict.problems:
        return verdict

    kind = bump_kind(old, new)
    if kind is None:
        verdict.problems.append(
            Problem(
                "not-a-bump",
                f"{old} -> {new} is not a forward bump: exactly one of major, minor or patch "
                f"must go up by one, with the parts below it zeroed",
            )
        )
        return verdict

    verdict.old, verdict.new, verdict.kind = old, new, kind
    return verdict


def check_head_versions(texts: dict[str, str | None], new: str) -> list[Problem]:
    """Every version line in the merged tree must read `new`.

    The diff alone cannot see this. `pixi.toml` carries the version twice —
    `[workspace]` and `[package]` — and `shelf.toml` a third time, and a PR
    that moves two of the three leaves the repo in the state `doctor` calls
    `version-mismatch`: `shelf publish` reads one file and pixi reads
    another, so the tin publishes under a version nothing else agrees with.
    Reading the files at the head revision catches that, including the case
    where the third file is simply absent from the diff.
    """
    problems: list[Problem] = []
    for name, text in texts.items():
        if text is None:
            continue  # the repo has no such file; nothing to disagree with
        for m in VERSION_LINE_RE.finditer(text):
            if (found := m.group("version")) != new:
                problems.append(
                    Problem(
                        "incomplete-bump",
                        f'{name} still reads version = "{found}" at the head revision; '
                        f"after this bump every version line must read {new}",
                    )
                )
    return problems
