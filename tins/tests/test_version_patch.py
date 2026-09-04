"""Tests for the version-bump validator behind `tins merge`.

These are the point of the command. `tins merge` exists so that nobody has
to read the fifth identical release diff as carefully as the first, which
means the validator carries the whole review — and a validator that lets one
smuggled source edit through is worse than no command at all, because it
turns an attentive habit into a rubber stamp.

So the cases below are mostly the refusals. The accepting fixture is the real
patch from magmalake/parquet.mojo#11 (0.4.2 -> 0.4.3, two files, three
lines), and each rejecting fixture is that same patch with one thing wrong.
"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from tins import versionpatch  # noqa: E402
from tins.versionpatch import ChangedFile  # noqa: E402

# --- fixtures -----------------------------------------------------------
#
# Taken verbatim from `gh api repos/magmalake/parquet.mojo/pulls/11/files`.

PIXI_PATCH = '''@@ -2,7 +2,7 @@
 authors = ["Marius S <39998+winding-lines@users.noreply.github.com>"]
 channels = ["https://conda.modular.com/max-nightly", "conda-forge"]
 name = "parquet.mojo"
-version = "0.4.2"
+version = "0.4.3"
 description = "Native pure-Mojo Apache Parquet."
 preview = ["pixi-build"]
 platforms = ["osx-arm64", "linux-64"]
@@ -64,7 +64,7 @@ lint-codecs = { cmd = "mojolint --lsp -I src" }
 # Conda package name is parquet-mojo; the import stays `from parquet import`.
 [package]
 name = "parquet-mojo"
-version = "0.4.2"
+version = "0.4.3"

 [package.build]
 backend = { name = "pixi-build-mojo", version = "0.*" }'''

SHELF_PATCH = '''@@ -1,5 +1,5 @@
 name = "parquet-mojo"
-version = "0.4.2"
+version = "0.4.3"
 description = "Native pure-Mojo Apache Parquet reader and writer."
 tags = ["parquet", "arrow", "iceberg", "data-lake", "columnar", "magmalake"]
 tins = ["thrift-mojo", "hashes-mojo", "snappy-mojo", "avro-mojo"]'''

SRC_PATCH = '''@@ -10,7 +10,7 @@ fn read(owned data: List[UInt8]) raises -> Table:
     var footer = parse_footer(data)
-    var pages = read_pages(footer)
+    var pages = read_pages(footer, parallel=True)
     return assemble(pages)'''

LOCK_PATCH = '''@@ -205,7 +205,7 @@ environments:
-      - conda: https://conda.anaconda.org/conda-forge/linux-64/libparquet-24.0.0-h7376487_9_cpu.conda
+      - conda: https://conda.anaconda.org/conda-forge/linux-64/libparquet-24.1.0-h7376487_9_cpu.conda'''

PIXI_HEAD = '''[workspace]
name = "parquet.mojo"
version = "0.4.3"

[package]
name = "parquet-mojo"
version = "0.4.3"
'''

SHELF_HEAD = '''name = "parquet-mojo"
version = "0.4.3"
tins = ["thrift-mojo"]
'''


def pixi(patch=PIXI_PATCH, status="modified"):
    return ChangedFile("pixi.toml", status, patch)


def shelf(patch=SHELF_PATCH, status="modified"):
    return ChangedFile("shelf.toml", status, patch)


def codes(verdict):
    return sorted({p.code for p in verdict.problems})


# --- the shape that must be accepted ------------------------------------


class TestAccepts(unittest.TestCase):
    def test_the_real_pure_version_bump(self):
        """parquet.mojo#11: two files, three lines, 0.4.2 -> 0.4.3."""
        v = versionpatch.check_changed_files([pixi(), shelf()])
        self.assertEqual(v.problems, [])
        self.assertTrue(v.ok)
        self.assertEqual((v.old, v.new, v.kind), ("0.4.2", "0.4.3", "patch"))
        self.assertEqual(v.lines, {"pixi.toml": 2, "shelf.toml": 1})

    def test_head_agrees_when_all_three_lines_moved(self):
        problems = versionpatch.check_head_versions(
            {"pixi.toml": PIXI_HEAD, "shelf.toml": SHELF_HEAD}, "0.4.3"
        )
        self.assertEqual(problems, [])

    def test_a_repo_without_a_shelf_toml_is_fine(self):
        """Absent is not disagreement: not every repo publishes a tin."""
        problems = versionpatch.check_head_versions(
            {"pixi.toml": PIXI_HEAD, "shelf.toml": None}, "0.4.3"
        )
        self.assertEqual(problems, [])


# --- only version files may change --------------------------------------


class TestForeignFiles(unittest.TestCase):
    def test_a_source_file_alongside_the_bump(self):
        """The case the command exists for: a real edit riding along."""
        v = versionpatch.check_changed_files(
            [pixi(), shelf(), ChangedFile("src/parquet/reader.mojo", "modified", SRC_PATCH)]
        )
        self.assertEqual(codes(v), ["foreign-file"])
        self.assertIn("src/parquet/reader.mojo", v.problems[0].message)
        self.assertFalse(v.ok)

    def test_pixi_lock(self):
        """A bump needs no relock; the lock does not carry the version."""
        v = versionpatch.check_changed_files(
            [pixi(), shelf(), ChangedFile("pixi.lock", "modified", LOCK_PATCH)]
        )
        self.assertEqual(codes(v), ["foreign-file"])
        self.assertIn("pixi.lock", v.problems[0].message)

    def test_ci_and_readme_are_no_different(self):
        for name in (".github/workflows/ci.yml", "README.md", "tins/pixi.toml"):
            with self.subTest(name=name):
                v = versionpatch.check_changed_files(
                    [pixi(), shelf(), ChangedFile(name, "modified", SRC_PATCH)]
                )
                self.assertEqual(codes(v), ["foreign-file"])

    def test_a_version_file_that_is_added_or_deleted_rather_than_edited(self):
        for status in ("added", "removed", "renamed", "changed"):
            with self.subTest(status=status):
                v = versionpatch.check_changed_files([pixi(status=status), shelf()])
                self.assertEqual(codes(v), ["foreign-file"])

    def test_an_empty_diff_is_not_a_bump(self):
        self.assertEqual(codes(versionpatch.check_changed_files([])), ["empty-diff"])


# --- every changed line must be a version line --------------------------


class TestChangedLines(unittest.TestCase):
    def test_an_unrelated_changed_line_inside_pixi_toml(self):
        """A smuggled edit in a file that is allowed to change."""
        patch = PIXI_PATCH.replace(
            ' preview = ["pixi-build"]',
            '-preview = ["pixi-build"]\n+preview = ["pixi-build", "pixi-build-backends"]',
        )
        v = versionpatch.check_changed_files([pixi(patch), shelf()])
        self.assertEqual(codes(v), ["non-version-line"])
        self.assertIn("preview", v.problems[0].message)

    def test_an_added_line_that_is_not_a_version_line(self):
        patch = PIXI_PATCH + '\n+exclude = ["tests"]'
        v = versionpatch.check_changed_files([pixi(patch), shelf()])
        self.assertEqual(codes(v), ["non-version-line"])

    def test_an_indented_version_line_is_not_the_packages_version(self):
        """Indented means it lives inside a dependency table, not the package."""
        patch = PIXI_PATCH.replace(
            '-version = "0.4.2"\n+version = "0.4.3"',
            '-  version = "0.4.2"\n+  version = "0.4.3"',
            1,
        )
        v = versionpatch.check_changed_files([pixi(patch), shelf()])
        self.assertEqual(codes(v), ["non-version-line"])

    def test_a_dependency_pin_that_happens_to_mention_a_version(self):
        patch = PIXI_PATCH.replace(
            ' name = "parquet-mojo"',
            '-thrift-mojo = { version = "0.4.2" }\n+thrift-mojo = { version = "0.4.3" }',
            1,
        )
        v = versionpatch.check_changed_files([pixi(patch), shelf()])
        self.assertEqual(codes(v), ["non-version-line"])

    def test_a_trailing_comment_on_a_version_line(self):
        """Not a shape `manifest.set_version` writes, so not one we trust."""
        patch = PIXI_PATCH.replace('+version = "0.4.3"', '+version = "0.4.3"  # bumped', 1)
        v = versionpatch.check_changed_files([pixi(patch), shelf()])
        self.assertEqual(codes(v), ["non-version-line"])

    def test_a_patch_github_would_not_render(self):
        v = versionpatch.check_changed_files([pixi(patch=None), shelf()])
        self.assertEqual(codes(v), ["unreadable-patch"])

    def test_a_diff_line_in_no_recognised_role(self):
        v = versionpatch.check_changed_files([pixi(PIXI_PATCH + "\nrogue text"), shelf()])
        self.assertEqual(codes(v), ["malformed-patch"])

    def test_content_before_the_first_hunk_header(self):
        v = versionpatch.check_changed_files([pixi("-version = \"0.4.2\"\n" + PIXI_PATCH), shelf()])
        self.assertEqual(codes(v), ["malformed-patch"])

    def test_an_unrecognised_hunk_header(self):
        v = versionpatch.check_changed_files([pixi(PIXI_PATCH.replace("@@ -2,7 +2,7 @@", "@@@ what")), shelf()])
        self.assertEqual(codes(v), ["malformed-patch"])

    def test_a_removed_version_line_with_no_replacement(self):
        patch = PIXI_PATCH.replace('+version = "0.4.3"\n', "", 1)
        v = versionpatch.check_changed_files([pixi(patch), shelf()])
        self.assertEqual(codes(v), ["unbalanced-change"])

    def test_a_version_file_modified_without_touching_a_version_line(self):
        patch = '@@ -1,3 +1,3 @@\n name = "parquet-mojo"\n tins = ["thrift-mojo"]'
        v = versionpatch.check_changed_files([pixi(), shelf(patch)])
        self.assertEqual(codes(v), ["no-version-change"])

    def test_the_end_of_file_marker_is_not_a_changed_line(self):
        v = versionpatch.check_changed_files(
            [pixi(), shelf(SHELF_PATCH + "\n\\ No newline at end of file")]
        )
        self.assertTrue(v.ok)


# --- one old version and one new version --------------------------------


class TestOneBump(unittest.TestCase):
    def test_two_files_disagreeing_about_the_new_version(self):
        v = versionpatch.check_changed_files(
            [pixi(), shelf(SHELF_PATCH.replace("0.4.3", "0.4.4"))]
        )
        self.assertEqual(codes(v), ["version-disagreement"])
        self.assertIn("0.4.4", v.problems[0].message)

    def test_two_occurrences_in_one_file_disagreeing(self):
        patch = PIXI_PATCH.replace('-version = "0.4.2"', '-version = "0.4.1"', 1)
        v = versionpatch.check_changed_files([pixi(patch), shelf()])
        self.assertEqual(codes(v), ["version-disagreement"])

    def test_a_bump_in_only_two_of_the_three_places(self):
        """pixi.toml twice and shelf.toml once; two of three is a bug.

        The diff cannot see this — shelf.toml simply is not in it — so it is
        the head-revision read that catches it. Left alone it becomes
        `doctor`'s version-mismatch: `shelf publish` reads one number and
        pixi another.
        """
        bump = versionpatch.check_changed_files([pixi()])
        self.assertTrue(bump.ok)  # the diff on its own looks perfect
        problems = versionpatch.check_head_versions(
            {"pixi.toml": PIXI_HEAD, "shelf.toml": SHELF_HEAD.replace("0.4.3", "0.4.2")},
            bump.new,
        )
        self.assertEqual([p.code for p in problems], ["incomplete-bump"])
        self.assertIn("shelf.toml", problems[0].message)

    def test_only_the_workspace_half_of_pixi_toml_moved(self):
        head = PIXI_HEAD.replace('version = "0.4.3"', 'version = "0.4.2"', 1)
        problems = versionpatch.check_head_versions(
            {"pixi.toml": head, "shelf.toml": SHELF_HEAD}, "0.4.3"
        )
        self.assertEqual([p.code for p in problems], ["incomplete-bump"])
        self.assertIn("0.4.2", problems[0].message)


# --- the new version must be a forward bump -----------------------------


class TestBumpKind(unittest.TestCase):
    def test_the_three_bumps(self):
        self.assertEqual(versionpatch.bump_kind("0.4.2", "0.4.3"), "patch")
        self.assertEqual(versionpatch.bump_kind("0.4.2", "0.5.0"), "minor")
        self.assertEqual(versionpatch.bump_kind("0.4.2", "1.0.0"), "major")
        self.assertEqual(versionpatch.bump_kind("0.9.9", "0.10.0"), "minor")

    def test_everything_that_is_not_a_bump(self):
        for old, new in [
            ("0.4.2", "0.4.2"),  # no change
            ("0.4.3", "0.4.2"),  # backwards
            ("0.4.2", "0.4.4"),  # a skipped patch
            ("0.4.2", "0.5.1"),  # minor up but patch not zeroed
            ("0.4.2", "1.0.1"),  # major up but patch not zeroed
            ("0.4.2", "1.1.0"),  # two parts at once
            ("0.4.2", "0.3.9"),  # minor backwards
        ]:
            with self.subTest(old=old, new=new):
                self.assertIsNone(versionpatch.bump_kind(old, new))

    def test_a_backwards_bump_in_a_real_diff(self):
        v = versionpatch.check_changed_files(
            [
                pixi(PIXI_PATCH.replace("0.4.2", "X").replace("0.4.3", "0.4.2").replace("X", "0.4.3")),
                shelf(SHELF_PATCH.replace("0.4.2", "X").replace("0.4.3", "0.4.2").replace("X", "0.4.3")),
            ]
        )
        self.assertEqual(codes(v), ["not-a-bump"])
        self.assertIn("0.4.3 -> 0.4.2", v.problems[0].message)

    def test_a_no_op_bump(self):
        v = versionpatch.check_changed_files(
            [pixi(PIXI_PATCH.replace("0.4.3", "0.4.2")), shelf(SHELF_PATCH.replace("0.4.3", "0.4.2"))]
        )
        self.assertEqual(codes(v), ["not-a-bump"])


class TestMalformedVersions(unittest.TestCase):
    def test_versions_the_bumper_could_never_have_written(self):
        for bad in ("0.5", "0.4.3-rc1", "0.4.03", "v0.4.3", "0.4.3.1", "", "latest"):
            with self.subTest(bad=bad):
                self.assertIsNone(versionpatch.parse_semver(bad))

    def test_a_malformed_new_version_in_a_real_diff(self):
        v = versionpatch.check_changed_files(
            [pixi(PIXI_PATCH.replace("0.4.3", "0.4.3-rc1")), shelf(SHELF_PATCH.replace("0.4.3", "0.4.3-rc1"))]
        )
        self.assertEqual(codes(v), ["malformed-version"])
        self.assertIn("0.4.3-rc1", v.problems[0].message)

    def test_a_malformed_old_version_in_a_real_diff(self):
        v = versionpatch.check_changed_files(
            [pixi(PIXI_PATCH.replace("0.4.2", "0.4")), shelf(SHELF_PATCH.replace("0.4.2", "0.4"))]
        )
        self.assertEqual(codes(v), ["malformed-version"])


if __name__ == "__main__":
    unittest.main()
