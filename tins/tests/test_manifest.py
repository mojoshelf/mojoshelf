"""Tests for the manifest rewrites — the only part of tins that edits files."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from tins import manifest  # noqa: E402
from tins.workspace import _collect_deps  # noqa: E402

PIXI = """\
[workspace]
name = "parquet.mojo"
version = "0.4.0"

[dependencies]
# Added by `pixi shelf add lint-mojo`.
lint-mojo = { git = "https://github.com/magmalake/lint.mojo.git", rev = "2a12807b7847984aff2c8bf148b0eb7515a99844" }

[package]
name = "parquet-mojo"
version = "0.4.0"

[package.host-dependencies]
mojo-compiler = "==1.0.0"
thrift-mojo = { git = "https://github.com/magmalake/thrift.mojo", rev = "5c474cbc4ccec1d98f5750598786d4e93d0fd931" }

[package.run-dependencies]
mojo-compiler = ">=1.0.0,<2"
thrift-mojo = { git = "https://github.com/magmalake/thrift.mojo", rev = "5c474cbc4ccec1d98f5750598786d4e93d0fd931" }

[feature.bench.dependencies]
bench-mojo = { git = "https://github.com/magmalake/bench.mojo", rev = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
"""


class TestSetDepRev(unittest.TestCase):
    def test_moves_every_occurrence(self):
        new, n = manifest.set_dep_rev(PIXI, "thrift-mojo", "b" * 40)
        self.assertEqual(n, 2, "host- and run-dependencies must move together")
        self.assertNotIn("5c474cbc", new)
        self.assertEqual(new.count("b" * 40), 2)

    def test_leaves_other_packages_alone(self):
        new, _ = manifest.set_dep_rev(PIXI, "thrift-mojo", "b" * 40)
        self.assertIn("2a12807b7847984aff2c8bf148b0eb7515a99844", new)
        self.assertIn("a" * 40, new)

    def test_keeps_comments_and_urls(self):
        new, _ = manifest.set_dep_rev(PIXI, "lint-mojo", "c" * 40)
        self.assertIn("# Added by `pixi shelf add lint-mojo`.", new)
        self.assertIn("https://github.com/magmalake/lint.mojo.git", new)

    def test_unknown_package_is_a_no_op(self):
        new, n = manifest.set_dep_rev(PIXI, "nope-mojo", "d" * 40)
        self.assertEqual((new, n), (PIXI, 0))

    def test_similar_name_is_not_a_prefix_match(self):
        _, n = manifest.set_dep_rev(PIXI, "mojo", "d" * 40)
        self.assertEqual(n, 0)


class TestSetVersion(unittest.TestCase):
    def test_rewrites_both_version_lines(self):
        new, n = manifest.set_version(PIXI, "0.4.0", "0.4.1")
        self.assertEqual(n, 2)
        self.assertEqual(new.count('version = "0.4.1"'), 2)

    def test_does_not_touch_dependency_specs(self):
        pixi = PIXI.replace('mojo-compiler = "==1.0.0"', 'mojo-compiler = "==0.4.0"')
        new, n = manifest.set_version(pixi, "0.4.0", "0.4.1")
        self.assertEqual(n, 2)
        self.assertIn('mojo-compiler = "==0.4.0"', new)


class TestBump(unittest.TestCase):
    def test_parts(self):
        self.assertEqual(manifest.bump("0.4.0"), "0.4.1")
        self.assertEqual(manifest.bump("0.4.9", "minor"), "0.5.0")
        self.assertEqual(manifest.bump("0.4.9", "major"), "1.0.0")

    def test_rejects_non_semver(self):
        with self.assertRaises(ValueError):
            manifest.bump("0.4")


class TestVersionKey(unittest.TestCase):
    def test_orders_numerically(self):
        versions = ["0.9.0", "0.10.0", "0.1.0"]
        self.assertEqual(
            sorted(versions, key=manifest.version_key), ["0.1.0", "0.9.0", "0.10.0"]
        )


class TestCollectDeps(unittest.TestCase):
    def test_finds_git_deps_and_labels_their_tables(self):
        import tomllib

        deps = {d.pkg: d for d in _collect_deps(tomllib.loads(PIXI))}
        self.assertEqual(
            set(deps), {"lint-mojo", "thrift-mojo", "bench-mojo"}
        )
        self.assertTrue(deps["thrift-mojo"].is_package_dep)
        self.assertFalse(deps["lint-mojo"].is_package_dep)
        self.assertFalse(deps["bench-mojo"].is_package_dep)


if __name__ == "__main__":
    unittest.main()
