"""The plan doctor prints, and the ordering that makes it worth printing."""

from __future__ import annotations

import contextlib
import io
import unittest
from argparse import Namespace
from dataclasses import dataclass, field

from tins import remedy
from tins.__main__ import build_parser
from tins.commands import ERROR, INFO, WARN, Finding


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


ARGS = Namespace(org=None, tins=False)


def plan_for(repos, findings, args=ARGS):
    return remedy.build(repos, findings, args)


def commands(plan) -> list[str]:
    return [c for step in plan.steps for c in step.commands]


class TestOrdering(unittest.TestCase):
    def test_a_disagreeing_version_is_normalised_before_anything_bumps_it(self):
        repos = [FakeRepo("magmalake", "a.mojo", "a"), FakeRepo("magmalake", "b.mojo", "b")]
        findings = [
            Finding("magmalake/b.mojo", ERROR, "stale-release", ""),
            Finding("magmalake/a.mojo", ERROR, "version-mismatch", ""),
        ]
        first = commands(plan_for(repos, findings))[0]
        self.assertIn(" fix ", first)
        self.assertIn("--repo a.mojo", first)

    def test_a_release_is_merged_and_published_in_that_order(self):
        repos = [FakeRepo("magmalake", "a.mojo", "a")]
        findings = [Finding("magmalake/a.mojo", ERROR, "stale-release", "")]
        self.assertEqual(
            commands(plan_for(repos, findings)),
            [
                "tins --repo a.mojo release --bump minor --yes",
                "tins --repo a.mojo merge --yes",
                "tins --repo a.mojo publish --yes",
            ],
        )


class TestTheHeldPublish(unittest.TestCase):
    """The whole reason to compute a plan rather than list remedies.

    A consumer sitting on an unpublished bump, whose dependency is about to
    release, must not publish yet: the pin it carries is about to go stale,
    and publishing now means re-pinning later, which owes it a second
    release. Held, the pin move lands in the release it has not made.
    """

    def setUp(self):
        self.repos = [
            FakeRepo("magmalake", "parquet.mojo", "parquet-mojo"),
            FakeRepo("magmalake", "iceberg.mojo", "iceberg-mojo", [FakeDep("parquet-mojo")]),
        ]
        self.findings = [
            Finding("magmalake/parquet.mojo", ERROR, "stale-release", ""),
            Finding("magmalake/iceberg.mojo", WARN, "unpublished", ""),
        ]

    def test_the_consumer_is_repinned_after_rather_than_published_before(self):
        cmds = commands(plan_for(self.repos, self.findings))
        # It is published -- but at the end of the repin chain, never before
        # the dependency it pins has released.
        self.assertLess(
            cmds.index("tins --repo iceberg.mojo repin"),
            cmds.index("tins --repo iceberg.mojo publish --yes"),
        )
        self.assertLess(
            cmds.index("tins --repo parquet.mojo release --bump minor --yes"),
            cmds.index("tins --repo iceberg.mojo repin"),
        )

    def test_the_saved_release_is_explained_rather_than_left_to_be_inferred(self):
        why = plan_for(self.repos, self.findings).steps[-1].why
        self.assertIn("iceberg.mojo", why)
        self.assertIn("saves it a second release", why)
        # Its pin is correct today -- step 2 is what invalidates it -- so it
        # must not be counted among the pins that are already wrong.
        self.assertNotIn("pinning a rev that is not the published one", why)

    def test_an_unrelated_unpublished_repo_still_publishes_immediately(self):
        """The negative control: holding is caused by the dependency, not by
        the mere presence of a stale-release finding somewhere."""
        repos = [*self.repos, FakeRepo("millfolio", "docx.mojo", "docx-mojo")]
        findings = [*self.findings, Finding("millfolio/docx.mojo", WARN, "unpublished", "")]
        cmds = commands(plan_for(repos, findings))
        self.assertIn("tins --repo docx.mojo publish --yes", cmds)
        self.assertLess(
            cmds.index("tins --repo iceberg.mojo repin"),
            cmds.index("tins --repo iceberg.mojo publish --yes"),
        )

    def test_a_transitive_consumer_is_held_too(self):
        repos = [*self.repos, FakeRepo("magmalake", "r.mojo", "r-mojo", [FakeDep("iceberg-mojo")])]
        findings = [*self.findings, Finding("magmalake/r.mojo", WARN, "unpublished", "")]
        cmds = commands(plan_for(repos, findings))
        self.assertNotIn("tins --repo r.mojo publish --yes", cmds[:3])
        self.assertIn("--repo r.mojo", [c for c in cmds if "repin" in c.split()][0])


class TestWhatCanBePublished(unittest.TestCase):
    def test_a_consumer_that_publishes_nothing_is_repinned_but_not_published(self):
        """Refinery takes pins and ships nothing. A publish naming it is a
        command that fails, which is worse than no suggestion at all."""
        repos = [
            FakeRepo("magmalake", "iceberg.mojo", "iceberg-mojo"),
            FakeRepo("magmalake", "Refinery", None, [FakeDep("iceberg-mojo")], publishable=False),
        ]
        findings = [Finding("magmalake/Refinery", WARN, "outdated-pin", "")]
        cmds = commands(plan_for(repos, findings))
        self.assertEqual(
            cmds, ["tins --repo Refinery repin", "tins --repo Refinery merge --yes"]
        )


class TestTheCommandsAreReal(unittest.TestCase):
    """A plan that prints commands the CLI rejects is worse than no plan.

    `--repo` and `--org` are global flags, so they go before the subcommand:
    `tins --repo x release`, never `tins release --repo x`. Nothing but
    feeding the output back through the real parser catches that.
    """

    def _parses(self, cmd: str) -> None:
        parser = build_parser()
        with contextlib.redirect_stderr(io.StringIO()):
            try:
                parser.parse_args(cmd.split()[1:])
            except SystemExit:
                self.fail(f"the CLI rejects a command the plan printed: {cmd}")

    def test_every_command_in_a_full_plan_parses(self):
        repos = [
            FakeRepo("magmalake", "parquet.mojo", "parquet-mojo"),
            FakeRepo("magmalake", "iceberg.mojo", "iceberg-mojo", [FakeDep("parquet-mojo")]),
            FakeRepo("millfolio", "docx.mojo", "docx-mojo"),
        ]
        findings = [
            Finding("magmalake/parquet.mojo", ERROR, "stale-release", ""),
            Finding("magmalake/iceberg.mojo", WARN, "unpublished", ""),
            Finding("millfolio/docx.mojo", ERROR, "version-mismatch", ""),
        ]
        cmds = commands(plan_for(repos, findings))
        self.assertTrue(cmds)
        for c in cmds:
            self._parses(c)

    def test_it_parses_the_wide_scope_form_too(self):
        repos = [FakeRepo("magmalake", f"r{i}.mojo", f"r{i}") for i in range(5)]
        findings = [Finding(r.slug, ERROR, "version-mismatch", "") for r in repos]
        for c in commands(plan_for(repos, findings, Namespace(org="magmalake", tins=True))):
            self._parses(c)


class TestScope(unittest.TestCase):
    def test_repos_are_named_so_a_pasted_command_cannot_overreach(self):
        repos = [FakeRepo("magmalake", "a.mojo", "a"), FakeRepo("magmalake", "b.mojo", "b")]
        findings = [Finding("magmalake/a.mojo", ERROR, "version-mismatch", "")]
        self.assertEqual(commands(plan_for(repos, findings)), ["tins --repo a.mojo fix --yes"])

    def test_past_four_repos_it_falls_back_to_the_scope_the_reader_typed(self):
        repos = [FakeRepo("magmalake", f"r{i}.mojo", f"r{i}") for i in range(5)]
        findings = [Finding(r.slug, ERROR, "version-mismatch", "") for r in repos]
        args = Namespace(org="magmalake", tins=True)
        self.assertEqual(
            commands(plan_for(repos, findings, args)), ["tins --org magmalake --tins fix --yes"]
        )


class TestWhatHasNoCommand(unittest.TestCase):
    def test_a_finding_with_no_command_says_so_rather_than_going_quiet(self):
        repos = [FakeRepo("magmalake", "a.mojo", "a")]
        findings = [Finding("magmalake/a.mojo", WARN, "shelf-tins-drift", "")]
        notes = plan_for(repos, findings).notes
        self.assertEqual(len(notes), 1)
        self.assertIn("shelf.toml", notes[0])

    def test_a_finding_kind_nobody_taught_the_planner_about_is_admitted(self):
        """Guards the failure that would otherwise be invisible: a new finding
        added to doctor, silently absent from the plan."""
        repos = [FakeRepo("magmalake", "a.mojo", "a")]
        findings = [Finding("magmalake/a.mojo", WARN, "some-future-check", "")]
        self.assertIn("some-future-check", plan_for(repos, findings).notes[0])

    def test_purely_informational_findings_produce_nothing(self):
        repos = [FakeRepo("magmalake", "a.mojo", "a")]
        findings = [Finding("magmalake/a.mojo", INFO, "unreleased-commits", "")]
        self.assertFalse(plan_for(repos, findings))


if __name__ == "__main__":
    unittest.main()
