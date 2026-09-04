"""Configuration: where the checkouts live, and the identity used to commit."""

from __future__ import annotations

import os
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
# Your own config, used when neither --config nor TINS_CONFIG is set. It is
# not in git: roots and identity are per-person.
DEFAULT_CONFIG = REPO_ROOT / "tins.toml"
EXAMPLE_CONFIG = REPO_ROOT / "tins.example.toml"


@dataclass
class Config:
    roots: list[Path] = field(default_factory=list)
    ignore: list[str] = field(default_factory=list)
    registry: str = "https://mojoshelf.org"
    pixi: str = "0.78.0"
    author: str = ""
    co_authored_by: str = ""

    @classmethod
    def load(cls, path: str | os.PathLike | None = None) -> "Config":
        p = Path(path or os.environ.get("TINS_CONFIG") or DEFAULT_CONFIG).expanduser()
        if not p.exists():
            if p == DEFAULT_CONFIG:
                raise SystemExit(
                    f"no config at {p}\n"
                    f"start from the example:  cp {EXAMPLE_CONFIG.name} {DEFAULT_CONFIG.name}"
                )
            raise SystemExit(f"no config at {p} (pass --config or set TINS_CONFIG)")
        data = tomllib.loads(p.read_text())
        defaults = data.get("defaults", {})
        discovery = data.get("discovery", {})
        return cls(
            roots=[Path(r).expanduser() for r in discovery.get("roots", [])],
            ignore=list(discovery.get("ignore", [])),
            registry=defaults.get("registry", "https://mojoshelf.org"),
            pixi=defaults.get("pixi", "0.78.0"),
            author=defaults.get("author", ""),
            co_authored_by=defaults.get("co_authored_by", ""),
        )

    @property
    def author_name(self) -> str:
        return self.author.rsplit(" <", 1)[0].strip()

    @property
    def author_email(self) -> str:
        return self.author.rsplit(" <", 1)[-1].rstrip(">").strip()

    def pixi_cmd(self, *args: str) -> list[str]:
        """A pixi invocation at the pinned version.

        The locally installed pixi is usually older than the one the repos'
        locks were written with, and a newer pixi rejects an older lock.
        """
        return ["pixi", "exec", "--spec", f"pixi=={self.pixi}", "--", "pixi", *args]
