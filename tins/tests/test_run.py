"""The guided loop, driven by scripted keystrokes.

What is worth pinning here is not the prompt wording but the two promises
the loop makes: nothing runs without a keystroke, and what runs is what was
displayed.
"""

from __future__ import annotations

import unittest
from argparse import Namespace
from dataclasses import dataclass, field
from unittest import mock

from tins import run
from tins.commands import ERROR, WARN, Finding


@dataclass
class FakeDep:
    pkg: str
    is_package_dep: bool = True


@dataclass
class FakeRepo:
    org: str
    name: str
    tin: str | None = None
    deps: list[FakeDep] = field(default_factory=list)
    publishable: bool = True

    @property
    def slug(self) -> str:
        return f"{self.org}/{self.name}"


ARGS = Namespace(org=None, repo=None, tins=False, no_fetch=False, verbose=False, config=None)

MISMATCH = [FakeRepo("millfolio", "docx.mojo", "docx-mojo")]
MISMATCH_FINDING = [Finding("millfolio/docx.mojo", ERROR, "version-mismatch", "")]


class Driver:
    """Scripted answers plus a record of every command actually executed."""

    def __init__(self, keys, states):
        self.keys = list(keys)
        self.states = list(states)
        self.ran: list[tuple[str, Namespace]] = []
        self.diagnosed = 0

    def input(self, _prompt=""):
        if not self.keys:
            raise AssertionError("the loop asked more questions than the test scripted")
        return self.keys.pop(0)

    def diagnose(self, _config, _args):
        self.diagnosed += 1
        return self.states[min(self.diagnosed - 1, len(self.states) - 1)]

    def record(self, name):
        def fn(args, _config):
            self.ran.append((name, args))
            return 0

        return fn

    @property
    def subcommands(self) -> list[str]:
        return [n for n, _ in self.ran]


def drive(driver, patches=("cmd_fix", "cmd_release", "cmd_merge", "cmd_publish", "cmd_repin")):
    stubs = {p: driver.record(p.removeprefix("cmd_")) for p in patches}
    with (
        mock.patch("builtins.input", driver.input),
        mock.patch("sys.stdin.isatty", return_value=True),
        mock.patch("tins.run.diagnose", driver.diagnose),
        mock.patch("tins.run._show_diffs"),
        mock.patch.multiple("tins.commands", **stubs),
    ):
        return run.cmd_run(ARGS, config=None)


class TestNothingRunsUnapproved(unittest.TestCase):
    def test_quitting_at_the_first_prompt_runs_nothing(self):
        d = Driver(["q"], [(MISMATCH, MISMATCH_FINDING)])
        self.assertEqual(drive(d), 0)
        self.assertEqual(d.ran, [])

    def test_a_non_terminal_is_refused_rather_than_defaulted_to_yes(self):
        with mock.patch("sys.stdin.isatty", return_value=False):
            self.assertEqual(run.cmd_run(ARGS, config=None), 2)

    def test_stopping_at_the_pr_prompt_leaves_the_merge_unrun(self):
        """The pull requests are open; the loop must not merge them anyway."""
        d = Driver(["y", "q"], [(MISMATCH, MISMATCH_FINDING)])
        self.assertEqual(drive(d), 0)
        self.assertEqual(d.subcommands, ["fix"])

    def test_end_of_input_is_a_quit_not_a_yes(self):
        d = Driver([], [(MISMATCH, MISMATCH_FINDING)])
        with mock.patch("builtins.input", side_effect=EOFError):
            with (
                mock.patch("sys.stdin.isatty", return_value=True),
                mock.patch("tins.run.diagnose", d.diagnose),
                mock.patch.multiple("tins.commands", cmd_fix=d.record("fix")),
            ):
                self.assertEqual(run.cmd_run(ARGS, config=None), 0)
        self.assertEqual(d.ran, [])


class TestWhatRunsIsWhatWasShown(unittest.TestCase):
    def test_the_executed_command_carries_the_scope_that_was_printed(self):
        d = Driver(["y", "y"], [(MISMATCH, MISMATCH_FINDING), (MISMATCH, [])])
        drive(d)
        self.assertEqual(d.subcommands, ["fix"])
        _, args = d.ran[0]
        self.assertEqual(args.repo, ["docx.mojo"])
        self.assertTrue(args.yes)

    def test_a_release_step_runs_release_then_merge_then_publish(self):
        repos = [FakeRepo("magmalake", "parquet.mojo", "parquet-mojo")]
        findings = [Finding("magmalake/parquet.mojo", ERROR, "stale-release", "")]
        d = Driver(["y", "y", "y"], [(repos, findings), (repos, [])])
        drive(d)
        self.assertEqual(d.subcommands, ["release", "merge", "publish"])
        self.assertEqual(d.ran[0][1].bump, "minor")


class TestThePlanIsRebuiltBetweenSteps(unittest.TestCase):
    def test_the_second_step_comes_from_a_fresh_diagnosis(self):
        """Each step changes what the next one should be. A plan computed
        once would describe a workspace that no longer exists."""
        first = (MISMATCH, MISMATCH_FINDING)
        second = ([FakeRepo("magmalake", "a.mojo", "a")], [Finding("magmalake/a.mojo", WARN, "unpublished", "")])
        d = Driver(["y", "y", "y"], [first, second, (MISMATCH, [])])
        drive(d)
        self.assertEqual(d.subcommands, ["fix", "publish"])
        self.assertGreaterEqual(d.diagnosed, 3)

    def test_a_command_that_fails_stops_the_loop_rather_than_continuing(self):
        def boom(_args, _config):
            return 3

        d = Driver(["y"], [(MISMATCH, MISMATCH_FINDING)])
        with (
            mock.patch("builtins.input", d.input),
            mock.patch("sys.stdin.isatty", return_value=True),
            mock.patch("tins.run.diagnose", d.diagnose),
            mock.patch.multiple("tins.commands", cmd_fix=boom, cmd_merge=d.record("merge")),
        ):
            self.assertEqual(run.cmd_run(ARGS, config=None), 3)
        self.assertEqual(d.ran, [])

    def test_a_clean_workspace_ends_the_loop(self):
        d = Driver([], [(MISMATCH, [])])
        self.assertEqual(drive(d), 0)
        self.assertEqual(d.ran, [])


if __name__ == "__main__":
    unittest.main()
