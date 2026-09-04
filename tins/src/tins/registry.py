"""Client for the mojoshelf registry.

`shelf info` prints the same facts but truncates commit shas to 12
characters; the JSON API carries the full sha, which is what a pin has to
match.
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from dataclasses import dataclass

from .manifest import version_key

# The registry sits behind Cloudflare, which answers the default
# Python-urllib agent with a 403.
USER_AGENT = "tins (+https://github.com/mojoshelf/tins)"


class RegistryError(RuntimeError):
    pass


@dataclass(frozen=True)
class Release:
    version: str
    sha: str


class Registry:
    def __init__(self, base: str = "https://mojoshelf.org", timeout: int = 20):
        self.base = base.rstrip("/")
        self.timeout = timeout
        self._cache: dict[str, dict | None] = {}

    def tin(self, name: str) -> dict | None:
        """The registry's record for `name`, or None if it has none.

        A transport failure raises rather than returning None: a network
        problem must not read as "nothing is published", which would make
        every pin look current and every version look unreleased.
        """
        if name in self._cache:
            return self._cache[name]
        req = urllib.request.Request(
            f"{self.base}/api/tins/{name}", headers={"User-Agent": USER_AGENT}
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as r:
                self._cache[name] = json.load(r)
        except urllib.error.HTTPError as e:
            if e.code == 404:
                self._cache[name] = None
            else:
                raise RegistryError(f"{self.base}/api/tins/{name}: HTTP {e.code}") from e
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as e:
            raise RegistryError(f"{self.base}/api/tins/{name}: {e}") from e
        return self._cache[name]

    def releases(self, name: str) -> list[Release]:
        tin = self.tin(name)
        if not tin:
            return []
        rels = [
            Release(v["version"], v["commit_sha"])
            for v in tin.get("versions", [])
            if v.get("version") and v.get("commit_sha")
        ]
        return sorted(rels, key=lambda r: version_key(r.version), reverse=True)

    def latest(self, name: str) -> Release | None:
        rels = self.releases(name)
        return rels[0] if rels else None

    def published(self, name: str, version: str) -> Release | None:
        return next((r for r in self.releases(name) if r.version == version), None)

    def known(self, name: str) -> bool:
        return self.tin(name) is not None
