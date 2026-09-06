"""Branch reuse, against real git repositories.

This is filesystem and git behaviour, so it is exercised on real repos in a
temporary directory rather than mocked. The bug it pins (mojoshelf#12) was
invisible to any fixture: a branch left over from an earlier run was reused
at its old base, and the pull request that came out conflicted with main.
"""

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tins.commands import _LEASES, _push, _worktree_on_branch


def git(cwd, *args):
    return subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, check=True
    ).stdout.strip()


class RepoFixture:
    """A repo with one commit on main, plus a second main commit to move to."""

    def __init__(self, root: Path):
        self.path = root / "repo"
        self.path.mkdir()
        git(self.path, "init", "--quiet", "-b", "main")
        git(self.path, "config", "user.email", "t@example.com")
        git(self.path, "config", "user.name", "t")
        (self.path / "pixi.toml").write_text('version = "0.1.0"\n')
        git(self.path, "add", "-A")
        git(self.path, "commit", "--quiet", "-m", "first")
        self.old_main = git(self.path, "rev-parse", "HEAD")
        (self.path / "src.mojo").write_text("var x = 1\n")
        git(self.path, "add", "-A")
        git(self.path, "commit", "--quiet", "-m", "second")
        self.new_main = git(self.path, "rev-parse", "HEAD")
        self.org, self.name = "magmalake", "repo"
        self._root = root

    def worktree_path(self, branch: str) -> Path:
        return self._root / f"repo.{branch}"


class TestBranchReuse(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = RepoFixture(Path(self._tmp.name))
        _LEASES.clear()

    def tearDown(self):
        self._tmp.cleanup()

    def stale_branch(self):
        """A branch left over from an earlier run, cut from the older main."""
        git(self.repo.path, "branch", "work", self.repo.old_main)

    def test_a_fresh_branch_is_cut_from_the_given_revision(self):
        wt = _worktree_on_branch(self.repo, "work", self.repo.new_main)
        self.assertEqual(git(wt, "rev-parse", "HEAD"), self.repo.new_main)

    def test_a_branch_whose_base_moved_is_reset_onto_it(self):
        """The bug: this used to come back at old_main, and every PR opened
        from it conflicted with main."""
        self.stale_branch()
        wt = _worktree_on_branch(self.repo, "work", self.repo.new_main)
        self.assertEqual(git(wt, "rev-parse", "HEAD"), self.repo.new_main)

    def test_the_old_tip_survives_the_reset(self):
        """A run may never destroy a commit outright."""
        self.stale_branch()
        _worktree_on_branch(self.repo, "work", self.repo.new_main)
        kept = git(self.repo.path, "rev-parse", f"refs/tins/stale/work/{self.repo.old_main[:8]}")
        self.assertEqual(kept, self.repo.old_main)

    def test_a_branch_already_on_the_revision_is_reused_not_reset(self):
        """The resume case, and the negative control for the reset: work
        committed on top of current main must survive a second run."""
        wt = _worktree_on_branch(self.repo, "work", self.repo.new_main)
        (wt / "pixi.toml").write_text('version = "0.1.1"\n')
        git(wt, "add", "-A")
        git(wt, "commit", "--quiet", "-m", "bump")
        committed = git(wt, "rev-parse", "HEAD")

        again = _worktree_on_branch(self.repo, "work", self.repo.new_main)
        self.assertEqual(git(again, "rev-parse", "HEAD"), committed)

    def test_an_existing_worktree_is_reset_in_place(self):
        self.stale_branch()
        git(self.repo.path, "worktree", "add", "--quiet",
            str(self.repo.worktree_path("work")), "work")
        wt = _worktree_on_branch(self.repo, "work", self.repo.new_main)
        self.assertEqual(git(wt, "rev-parse", "HEAD"), self.repo.new_main)
        self.assertTrue((wt / "src.mojo").is_file())  # the newer main's file is there


class TestThePushLease(unittest.TestCase):
    """A reset branch cannot fast-forward onto the remote it left behind, so
    the push needs a lease. Without one it is rejected and the PR keeps
    showing the older commit."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = RepoFixture(Path(self._tmp.name))
        _LEASES.clear()

    def tearDown(self):
        self._tmp.cleanup()

    def test_resetting_a_branch_records_the_remote_tip_as_the_lease(self):
        git(self.repo.path, "branch", "work", self.repo.old_main)
        with mock.patch("tins.commands._remote_tip", return_value="deadbeef"):
            _worktree_on_branch(self.repo, "work", self.repo.new_main)
        self.assertEqual(_LEASES[(str(self.repo.path), "work")], "deadbeef")

    def test_a_branch_that_did_not_need_resetting_takes_no_lease(self):
        with mock.patch("tins.commands._remote_tip", return_value="deadbeef"):
            _worktree_on_branch(self.repo, "work", self.repo.new_main)
        self.assertEqual(_LEASES, {})

    def test_the_lease_reaches_git(self):
        """The lease is only worth recording if the push carries it."""
        _LEASES[(str(self.repo.path), "work")] = "deadbeef"
        with mock.patch("tins.commands.gitutil.push") as pushed:
            _push(self.repo.path, self.repo, "work")
        self.assertEqual(pushed.call_args.kwargs["force_with_lease"], "deadbeef")

    def test_a_push_with_no_lease_does_not_force(self):
        with mock.patch("tins.commands.gitutil.push") as pushed:
            _push(self.repo.path, self.repo, "work")
        self.assertIsNone(pushed.call_args.kwargs["force_with_lease"])

    def test_a_lease_is_spent_once(self):
        """A second push of the same branch in one run must not still be
        carrying permission to overwrite the remote."""
        _LEASES[(str(self.repo.path), "work")] = "deadbeef"
        with mock.patch("tins.commands.gitutil.push") as pushed:
            _push(self.repo.path, self.repo, "work")
            _push(self.repo.path, self.repo, "work")
        self.assertIsNone(pushed.call_args.kwargs["force_with_lease"])

    def test_a_branch_with_no_remote_yet_takes_no_lease(self):
        git(self.repo.path, "branch", "work", self.repo.old_main)
        with mock.patch("tins.commands._remote_tip", return_value=None):
            _worktree_on_branch(self.repo, "work", self.repo.new_main)
        self.assertEqual(_LEASES, {})


if __name__ == "__main__":
    unittest.main()
