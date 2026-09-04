"""Tests for the rules `tins merge` applies around the diff.

The patch parser is tested next door. These are the other four rules — the
head-revision read, registry coherence, CI, and whether GitHub would let the
merge happen — pinned against stubbed GitHub and registry answers, so that a
verdict never depends on what happens to be open today.

The stubs replace only the two network seams (`gitutil` and `Registry`); the
rule code under test is the real thing.
"""

import sys
import unittest
from argparse import Namespace
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from tins import commands  # noqa: E402
from tins.registry import Release  # noqa: E402
from tins.workspace import Repo  # noqa: E402

from test_version_patch import PIXI_HEAD, PIXI_PATCH, SHELF_HEAD, SHELF_PATCH  # noqa: E402

FILES = [
    {"filename": "pixi.toml", "status": "modified", "patch": PIXI_PATCH},
    {"filename": "shelf.toml", "status": "modified", "patch": SHELF_PATCH},
]
HEADS = {"pixi.toml": PIXI_HEAD, "shelf.toml": SHELF_HEAD}
GREEN = [{"__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": "SUCCESS"}]


def a_pr(**over):
    pr = {
        "number": 11,
        "title": "Release 0.4.3",
        "url": "https://github.com/magmalake/parquet.mojo/pull/11",
        "headRefOid": "a893762d4d61e7139aa9dc86df6821fbfe157fad",
        "baseRefName": "main",
        "isDraft": False,
        "mergeable": "MERGEABLE",
        "mergeStateStatus": "CLEAN",
        "statusCheckRollup": GREEN,
    }
    pr.update(over)
    return pr


class FakeRegistry:
    """A registry that publishes parquet-mojo 0.4.2 and nothing newer."""

    def __init__(self, latest="0.4.2", known=True):
        self._latest, self._known = latest, known

    def known(self, tin):
        return self._known

    def latest(self, tin):
        return Release(self._latest, "0" * 40) if self._latest else None

    def published(self, tin, version):
        return Release(version, "0" * 40) if version == self._latest else None


class RuleCase(unittest.TestCase):
    def judge(self, pr=None, files=FILES, heads=None, registry=None, allow_no_checks=False):
        repo = Repo(
            path=Path("/nowhere"),
            org="magmalake",
            name="parquet.mojo",
            pixi={},
            pixi_text="",
            shelf={"name": "parquet-mojo"},
        )
        heads = HEADS if heads is None else heads
        with mock.patch.object(commands.gitutil, "pr_files", return_value=files), \
             mock.patch.object(commands.gitutil, "file_at_ref",
                               side_effect=lambda o, n, path, ref: heads.get(path)):
            return commands._judge_pr(
                repo,
                pr or a_pr(),
                registry or FakeRegistry(),
                Namespace(allow_no_checks=allow_no_checks),
            )

    def codes(self, verdict, level=commands.ERROR):
        return [f.code for f in verdict.notes if f.level == level]


class TestTheAcceptingCase(RuleCase):
    def test_a_pure_bump_with_green_ci_is_merged(self):
        v = self.judge()
        self.assertEqual(self.codes(v), [])
        self.assertTrue(v.ok)
        self.assertEqual(v.key, "magmalake/parquet.mojo#11")
        self.assertEqual(
            self.codes(v, commands.INFO), ["version-bump", "checks", "mergeable"]
        )


class TestHeadRevision(RuleCase):
    def test_a_version_line_left_behind_at_head(self):
        """Two of the three places bumped: the diff is clean, the tree is not."""
        v = self.judge(heads={"pixi.toml": PIXI_HEAD, "shelf.toml": SHELF_HEAD.replace("3", "2")})
        self.assertEqual(self.codes(v), ["incomplete-bump"])

    def test_an_unreadable_head_is_a_refusal_not_a_shrug(self):
        v = self.judge(heads={})
        self.assertEqual(self.codes(v), ["unreadable-head"])

    def test_an_unreadable_file_list_is_a_refusal(self):
        v = self.judge(files=None)
        self.assertEqual(self.codes(v), ["unreadable-diff"])


class TestRegistryCoherence(RuleCase):
    def test_a_new_version_already_on_the_registry_is_refused(self):
        """`tins publish` skips it, so merging would strand the release."""
        v = self.judge(registry=FakeRegistry(latest="0.4.3"))
        self.assertIn("already-published", self.codes(v))
        self.assertFalse(v.ok)

    def test_an_old_version_the_registry_does_not_publish_is_only_a_warning(self):
        """A repo can legitimately be several bumps ahead of the registry."""
        v = self.judge(registry=FakeRegistry(latest="0.3.9"))
        self.assertEqual(self.codes(v), [])
        self.assertEqual(self.codes(v, commands.WARN), ["registry-behind"])
        self.assertTrue(v.ok)

    def test_an_unregistered_tin_is_neither(self):
        v = self.judge(registry=FakeRegistry(known=False))
        self.assertTrue(v.ok)
        self.assertEqual(self.codes(v, commands.WARN), [])


class TestChecks(RuleCase):
    def test_a_failing_check(self):
        rollup = GREEN + [
            {"__typename": "CheckRun", "name": "macos", "status": "COMPLETED", "conclusion": "FAILURE"}
        ]
        v = self.judge(a_pr(statusCheckRollup=rollup))
        self.assertEqual(self.codes(v), ["checks-failing"])

    def test_a_check_still_running(self):
        rollup = GREEN + [
            {"__typename": "CheckRun", "name": "macos", "status": "IN_PROGRESS", "conclusion": None}
        ]
        v = self.judge(a_pr(statusCheckRollup=rollup))
        self.assertEqual(self.codes(v), ["checks-pending"])

    def test_no_checks_at_all_is_refused_by_default(self):
        """An empty rollup is also how a PR looks before its workflows start."""
        v = self.judge(a_pr(statusCheckRollup=[]))
        self.assertEqual(self.codes(v), ["no-checks"])
        v = self.judge(a_pr(statusCheckRollup=[]), allow_no_checks=True)
        self.assertEqual(self.codes(v), [])
        self.assertEqual(self.codes(v, commands.WARN), ["no-checks"])

    def test_skipped_and_neutral_are_not_failures(self):
        for conclusion in ("SKIPPED", "NEUTRAL"):
            with self.subTest(conclusion=conclusion):
                rollup = [{"__typename": "CheckRun", "name": "x", "status": "COMPLETED",
                           "conclusion": conclusion}]
                self.assertEqual(commands._check_state(rollup)[0], "green")

    def test_a_commit_status_from_another_service(self):
        self.assertEqual(
            commands._check_state([{"__typename": "StatusContext", "context": "cov",
                                    "state": "SUCCESS"}])[0],
            "green",
        )
        self.assertEqual(
            commands._check_state([{"__typename": "StatusContext", "context": "cov",
                                    "state": "FAILURE"}])[0],
            "failing",
        )
        self.assertEqual(
            commands._check_state([{"__typename": "StatusContext", "context": "cov",
                                    "state": "PENDING"}])[0],
            "pending",
        )

    def test_a_check_in_no_recognised_shape_counts_as_failing(self):
        self.assertEqual(commands._check_state([{"name": "mystery"}])[0], "failing")


class TestMergeability(RuleCase):
    def test_github_states_that_block_a_merge(self):
        for state in ("DIRTY", "BLOCKED", "BEHIND", "UNKNOWN"):
            with self.subTest(state=state):
                v = self.judge(a_pr(mergeStateStatus=state))
                self.assertEqual(self.codes(v), ["not-mergeable"])

    def test_an_unknown_mergeable_flag_is_a_refusal(self):
        """Closed PRs and freshly pushed ones both report UNKNOWN."""
        v = self.judge(a_pr(mergeable="UNKNOWN", mergeStateStatus="UNKNOWN"))
        self.assertEqual(self.codes(v), ["not-mergeable"])

    def test_conflicting(self):
        v = self.judge(a_pr(mergeable="CONFLICTING", mergeStateStatus="DIRTY"))
        self.assertEqual(self.codes(v), ["not-mergeable"])

    def test_unstable_is_left_to_the_check_rule(self):
        """UNSTABLE only ever means a non-required check is unhappy."""
        v = self.judge(a_pr(mergeStateStatus="UNSTABLE"))
        self.assertEqual(self.codes(v), [])

    def test_a_draft(self):
        v = self.judge(a_pr(isDraft=True))
        self.assertIn("draft", self.codes(v))

    def test_a_pull_request_that_does_not_target_main(self):
        v = self.judge(a_pr(baseRefName="release-0.5"))
        self.assertIn("wrong-base", self.codes(v))


class TestNoisyRefusalsAreSummarised(RuleCase):
    def test_only_the_first_few_of_one_rule_are_listed(self):
        patch = "@@ -1,1 +1,1 @@\n" + "\n".join(f"-pin{i} = 1" for i in range(10))
        v = self.judge(files=[{"filename": "pixi.toml", "status": "modified", "patch": patch}])
        listed = [f for f in v.notes if f.code == "non-version-line"]
        self.assertEqual(len(listed), commands.PROBLEMS_PER_CODE + 1)
        self.assertIn("and 7 more", listed[-1].message)


if __name__ == "__main__":
    unittest.main()
