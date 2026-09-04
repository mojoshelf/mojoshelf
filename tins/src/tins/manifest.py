"""Reading and editing pixi.toml / shelf.toml.

Reads go through tomllib. Writes are line-surgical rather than a
round-trip through a TOML writer: these manifests carry comments that
explain why a pin is what it is, and a round-trip would drop them.
"""

from __future__ import annotations

import re

# `name = { git = "...", rev = "<sha>" }` on one line, which is how both
# `pixi add --git` and `pixi shelf add` write it.
def _rev_re(pkg: str) -> re.Pattern[str]:
    return re.compile(
        rf'(?P<head>^[ \t]*{re.escape(pkg)}[ \t]*=[ \t]*\{{[^}}\n]*?rev[ \t]*=[ \t]*")'
        rf"(?P<rev>[0-9a-fA-F]{{7,40}})(?P<tail>\")",
        re.M,
    )


def _version_re(version: str) -> re.Pattern[str]:
    return re.compile(rf'^(?P<head>version[ \t]*=[ \t]*"){re.escape(version)}(?P<tail>")', re.M)


def set_dep_rev(text: str, pkg: str, rev: str) -> tuple[str, int]:
    """Repoint every pin of `pkg` at `rev`. Returns (text, replacements).

    A tin usually pins the same sibling twice — once under
    host-dependencies and once under run-dependencies — and both must move
    together, so every occurrence is replaced.
    """
    new, n = _rev_re(pkg).subn(rf"\g<head>{rev}\g<tail>", text)
    return new, n


def set_version(text: str, old: str, new: str) -> tuple[str, int]:
    """Rewrite bare `version = "old"` lines. Leaves dependency specs alone."""
    out, n = _version_re(old).subn(rf"\g<head>{new}\g<tail>", text)
    return out, n


def bump(version: str, part: str = "patch") -> str:
    nums = version.split(".")
    if len(nums) != 3 or not all(n.isdigit() for n in nums):
        raise ValueError(f"not a three-part version: {version!r}")
    major, minor, patch = (int(n) for n in nums)
    if part == "major":
        return f"{major + 1}.0.0"
    if part == "minor":
        return f"{major}.{minor + 1}.0"
    if part == "patch":
        return f"{major}.{minor}.{patch + 1}"
    raise ValueError(f"unknown version part: {part!r}")


def version_key(version: str) -> tuple:
    """Sort key that orders 0.10.0 after 0.9.0."""
    return tuple(int(p) if p.isdigit() else p for p in version.split("."))
