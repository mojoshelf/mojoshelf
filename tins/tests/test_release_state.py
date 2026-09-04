"""Tests for the stale-release check, against a real git repository.

The check exists because a merged change that never gets published reaches
nobody, and nothing else notices: the version string stays valid while the
published tree falls behind main. It has to tell that apart from a README or
CI commit, which moves the sha without changing a byte a consumer installs.
"""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from tins import commands  # noqa: E402
from tins.workspace import Repo  # noqa: E402


def _git(cwd, *args):
    subprocess.run(
        ["git", "-c", "user.name=t", "-c", "user.email=t@t", *args],
        cwd=cwd, check=True, capture_output=True, text=True,
    )


class TestChangedPaths(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.path = Path(self._tmp.name)
        _git(self.path, "init", "-q", "-b", "main")
        (self.path / "src").mkdir()
        (self.path / "src" / "lib.mojo").write_text("v1\n")
        (self.path / "README.md").write_text("docs\n")
        _git(self.path, "add", "-A")
        _git(self.path, "commit", "-qm", "initial")
        self.base = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=self.path, capture_output=True, text=True
        ).stdout.strip()
        self.repo = Repo(path=self.path, org="o", name="r", pixi={}, pixi_text="")

    def tearDown(self):
        self._tmp.cleanup()

    def _commit(self, rel, content):
        f = self.path / rel
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_text(content)
        _git(self.path, "add", "-A")
        _git(self.path, "commit", "-qm", f"touch {rel}")
        return subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=self.path, capture_output=True, text=True
        ).stdout.strip()

    def test_src_change_is_reported(self):
        head = self._commit("src/lib.mojo", "v2\n")
        changed = commands._changed_paths(self.repo, self.base, head)
        self.assertEqual(changed, ["src/lib.mojo"])
        self.assertTrue([p for p in changed if p.startswith("src/")])

    def test_docs_and_ci_changes_carry_no_src_path(self):
        self._commit("README.md", "more docs\n")
        head = self._commit(".github/workflows/ci.yml", "on: push\n")
        changed = commands._changed_paths(self.repo, self.base, head)
        self.assertEqual(sorted(changed), [".github/workflows/ci.yml", "README.md"])
        self.assertEqual([p for p in changed if p.startswith("src/")], [])

    def test_identical_revisions_report_nothing(self):
        self.assertEqual(commands._changed_paths(self.repo, self.base, self.base), [])

    def test_unknown_revision_reports_nothing_rather_than_raising(self):
        """A rev we cannot see must not become a confident wrong answer."""
        self.assertEqual(commands._changed_paths(self.repo, "0" * 40, self.base), [])


if __name__ == "__main__":
    unittest.main()


class TestBodyFile(unittest.TestCase):
    """--body-file must fail before any repo is touched, not mid-sweep."""

    def test_missing_file_is_a_clean_exit_not_a_traceback(self):
        with self.assertRaises(SystemExit) as cm:
            commands._read_body("/nonexistent/body.md", None)
        self.assertIn("no such body file", str(cm.exception))

    def test_file_contents_are_returned(self):
        with tempfile.NamedTemporaryFile("w", suffix=".md", delete=False) as f:
            f.write("hello\n")
            name = f.name
        try:
            self.assertEqual(commands._read_body(name, None), "hello\n")
        finally:
            Path(name).unlink()

    def test_body_flag_is_the_fallback_and_empty_is_allowed(self):
        self.assertEqual(commands._read_body(None, "inline"), "inline")
        self.assertEqual(commands._read_body(None, None), "")
