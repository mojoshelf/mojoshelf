"""What counts as a change a consumer installs.

`stale-release` used to look only under `src/`. A dependency pin lives in
pixi.toml, and moving one changes what every consumer resolves just as
surely as editing the code — so a pin move that reached main was reported
as `unreleased-commits`, "no release owed", which was wrong.
"""

from __future__ import annotations

import unittest
from unittest import mock

from tins.commands import _packaged_changes, _packaged_dependencies

PINNED = '''
[package.run-dependencies]
parquet-mojo = { git = "https://github.com/magmalake/parquet.mojo", rev = "%s" }

[dependencies]
bench-mojo = { git = "https://github.com/magmalake/bench.mojo", rev = "cafe0001" }

[tasks]
test = "mojo run tests/main.mojo"

[workspace]
version = "%s"
'''

OLD, NEW = "d6cd38b9", "e16ce8e1"


def manifest(rev=OLD, version="0.6.6", dev_rev="cafe0001"):
    return (PINNED % (rev, version)).replace("cafe0001", dev_rev)


def changes(before, after, changed=("pixi.toml",)):
    with mock.patch(
        "tins.commands.read_at_ref", side_effect=lambda _p, _f, ref: before if ref == "old" else after
    ):
        return _packaged_changes(mock.Mock(path="/tmp"), "old", "new", list(changed))


class TestAPinMoveIsPackaged(unittest.TestCase):
    def test_moving_a_packaged_pin_owes_a_release(self):
        found = changes(manifest(rev=OLD), manifest(rev=NEW))
        self.assertEqual(found, ["pixi.toml (packaged dependencies)"])

    def test_it_is_found_even_though_nothing_under_src_changed(self):
        """The exact shape of a `tins repin` PR: one file, not in src/."""
        self.assertTrue(changes(manifest(rev=OLD), manifest(rev=NEW), changed=("pixi.toml",)))


class TestWhatStillOwesNothing(unittest.TestCase):
    """The other half of the rule. Counting every pixi.toml edit would cry
    wolf on every repo, which is why the filename alone is not enough."""

    def test_a_version_bump_alone_is_not_a_packaged_change(self):
        self.assertEqual(changes(manifest(version="0.6.6"), manifest(version="0.6.7")), [])

    def test_a_development_dependency_is_not_a_packaged_change(self):
        """`[dependencies]` is the workspace's own environment — a linter
        added there reaches no consumer."""
        self.assertEqual(changes(manifest(dev_rev="cafe0001"), manifest(dev_rev="beef0002")), [])

    def test_an_identical_manifest_is_not_a_change(self):
        self.assertEqual(changes(manifest(), manifest()), [])

    def test_an_unparsable_manifest_says_nothing_rather_than_inventing_a_release(self):
        self.assertEqual(changes(manifest(), "this is not toml ["), [])

    def test_a_manifest_that_cannot_be_read_says_nothing(self):
        self.assertEqual(changes(manifest(), None), [])

    def test_src_still_counts_on_its_own(self):
        found = changes(manifest(), manifest(), changed=("src/reader.mojo", "README.md"))
        self.assertEqual(found, ["src/reader.mojo"])


class TestWhichTablesCount(unittest.TestCase):
    def test_only_package_dependency_tables_are_read(self):
        tables = _packaged_dependencies(manifest())
        self.assertEqual(list(tables), ["run-dependencies"])


if __name__ == "__main__":
    unittest.main()
