"""The pin-move rules: what `tins merge` will accept from `tins repin`."""

from __future__ import annotations

import unittest

from tins.versionpatch import ChangedFile, check_pin_move

OLD = "d6cd38b96d3370b313ce7913a4a1645d6e46e663"
NEW = "e16ce8e1c9364b745d30b7fde094984b066818ea"
URL = "https://github.com/magmalake/parquet.mojo"


def pin_patch(old=OLD, new=NEW, pkg="parquet-mojo", tables=2) -> str:
    body = "".join(
        f'-{pkg} = {{ git = "{URL}", rev = "{old}" }}\n'
        f'+{pkg} = {{ git = "{URL}", rev = "{new}" }}\n'
        for _ in range(tables)
    )
    return "@@ -1,4 +1,4 @@\n" + body


def files(patch: str, name: str = "pixi.toml") -> list[ChangedFile]:
    return [ChangedFile(name, "modified", patch)]


def registry(*revs):
    return lambda _pkg: set(revs)


class TestAVerifiedPinMove(unittest.TestCase):
    def test_a_move_to_a_published_revision_is_accepted(self):
        v = check_pin_move(files(pin_patch()), registry(NEW))
        self.assertTrue(v.ok, v.problems)
        self.assertEqual(v.moves, {"parquet-mojo": (OLD, NEW)})

    def test_the_same_pin_in_several_tables_is_one_move(self):
        v = check_pin_move(files(pin_patch(tables=3)), registry(NEW))
        self.assertTrue(v.ok, v.problems)
        self.assertEqual(v.lines, {"pixi.toml": 6})


def bump_patch(old="0.6.6", new="0.6.7"):
    return f'@@ -1,2 +1,2 @@\n-version = "{old}"\n+version = "{new}"\n'


class TestABumpRidingAlong(unittest.TestCase):
    """`repin` bumps the version when a packaged pin moves, so the two
    arrive in one diff. The bump is held to the release rules anyway --
    otherwise a pin move is a way to smuggle any version past the
    validator."""

    def test_a_pin_move_with_a_forward_bump_is_accepted(self):
        v = check_pin_move(
            files(pin_patch()) + files(bump_patch(), "shelf.toml"), registry(NEW)
        )
        self.assertTrue(v.ok, v.problems)
        self.assertEqual(v.bump, ("0.6.6", "0.6.7", "patch"))

    def test_a_version_going_backwards_is_refused(self):
        v = check_pin_move(
            files(pin_patch()) + files(bump_patch("0.6.6", "0.6.5"), "shelf.toml"), registry(NEW)
        )
        self.assertFalse(v.ok)
        self.assertEqual([p.code for p in v.problems], ["not-a-bump"])

    def test_a_version_skipping_ahead_is_refused(self):
        v = check_pin_move(
            files(pin_patch()) + files(bump_patch("0.6.6", "0.9.0"), "shelf.toml"), registry(NEW)
        )
        self.assertFalse(v.ok)
        self.assertEqual([p.code for p in v.problems], ["not-a-bump"])

    def test_two_different_new_versions_are_refused(self):
        v = check_pin_move(
            files(pin_patch())
            + files(bump_patch(), "shelf.toml")
            + [ChangedFile("pixi.toml", "modified", bump_patch("0.6.6", "0.7.0"))],
            registry(NEW),
        )
        self.assertFalse(v.ok)
        self.assertIn("ambiguous-version", [p.code for p in v.problems])


class TestWhatItRefuses(unittest.TestCase):
    """The rules that make accepting a forty-character hex string safe."""

    def test_a_move_to_an_unpublished_revision_is_refused(self):
        """The whole proof. Without this the validator accepts any sha, and
        `unpublished-pin` -- the defect that has broken installs twice --
        gets merged rather than caught."""
        v = check_pin_move(files(pin_patch(new="0" * 40)), registry(NEW))
        self.assertFalse(v.ok)
        self.assertEqual([p.code for p in v.problems], ["unpublished-pin-target"])

    def test_an_unreachable_registry_refuses_rather_than_assumes(self):
        v = check_pin_move(files(pin_patch()), lambda _pkg: None)
        self.assertFalse(v.ok)
        self.assertEqual([p.code for p in v.problems], ["unverifiable-pin"])

    def test_a_source_edit_smuggled_alongside_a_pin_move_is_refused(self):
        patch = pin_patch() + '-var x = 1\n+var x = evil()\n'
        v = check_pin_move(files(patch), registry(NEW))
        self.assertFalse(v.ok)
        self.assertIn("non-pin-line", [p.code for p in v.problems])

    def test_a_file_that_is_not_a_manifest_is_refused(self):
        v = check_pin_move(files(pin_patch(), name="src/thing.mojo"), registry(NEW))
        self.assertFalse(v.ok)
        self.assertEqual([p.code for p in v.problems], ["foreign-file"])

    def test_an_unreadable_patch_is_refused(self):
        v = check_pin_move([ChangedFile("pixi.toml", "modified", None)], registry(NEW))
        self.assertFalse(v.ok)
        self.assertEqual([p.code for p in v.problems], ["unreadable-patch"])

    def test_two_destinations_for_one_pin_is_refused(self):
        patch = (
            "@@ -1,4 +1,4 @@\n"
            f'-parquet-mojo = {{ git = "{URL}", rev = "{OLD}" }}\n'
            f'+parquet-mojo = {{ git = "{URL}", rev = "{NEW}" }}\n'
            f'-parquet-mojo = {{ git = "{URL}", rev = "{OLD}" }}\n'
            f'+parquet-mojo = {{ git = "{URL}", rev = "{"a" * 40}" }}\n'
        )
        v = check_pin_move(files(patch), registry(NEW, "a" * 40))
        self.assertFalse(v.ok)
        self.assertEqual([p.code for p in v.problems], ["ambiguous-pin-move"])

    def test_a_diff_with_no_pin_move_is_refused(self):
        v = check_pin_move(files('@@ -1,2 +1,2 @@\n-version = "1.0.0"\n+version = "1.0.1"\n'), registry(NEW))
        self.assertFalse(v.ok)
        self.assertEqual([p.code for p in v.problems], ["no-pin-move"])


if __name__ == "__main__":
    unittest.main()
