"""The changelog entry a release owes.

iceberg.mojo released 0.7.0 off four merged pull requests. Three wrote their
own `[Unreleased]` entry; the fourth — the largest user-visible change in the
release — merged with none, and the only thing that caught it was somebody
diffing the commit log against the changelog by hand before rolling the
version. That is the review this check replaces.

The hard part is not noticing an empty section, it is not firing on the
nineteen repos that have no changelog on purpose, and not firing on a repo
that has already done the right thing — rolled `[Unreleased]` under the
version it is about to publish, which leaves `[Unreleased]` legitimately
empty right up to the bump.
"""

from __future__ import annotations

import unittest
from unittest import mock

from tins.commands import _changelog_gap, _describes_unreleased_work, _has_entries

PREAMBLE = """# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Releases before 0.6.0 predate this file; their contents are in the commit log
(each release is one commit whose subject begins with its version).
"""

PUBLISHED = """## [0.6.7] - 2026-09-06

### Fixed
- A projection that dropped a column.

## [0.6.0] - 2026-09-02

### Added
- The first release with a changelog.
"""


def changelog(*sections: str) -> str:
    return PREAMBLE + "\n" + "\n".join(sections) + "\n" + PUBLISHED


def gap(text: str | None, published: str = "0.6.7") -> str | None:
    """The finding message for a repo owed a release, or None if it is clear."""
    with mock.patch("tins.commands.read_at_ref", return_value=text):
        return _changelog_gap(mock.Mock(path="/tmp", ref="deadbeef"), published)


class TestWhatCountsAsAnEntry(unittest.TestCase):
    """`[Unreleased]` present but saying nothing is the case that bites."""

    def test_an_unreleased_section_with_a_bullet_is_an_entry(self):
        self.assertTrue(
            _describes_unreleased_work(
                changelog("## [Unreleased]\n\n### Added\n- A streaming path for a scan.\n"),
                "0.6.7",
            )
        )

    def test_an_empty_unreleased_section_is_not(self):
        self.assertFalse(_describes_unreleased_work(changelog("## [Unreleased]\n"), "0.6.7"))

    def test_a_section_of_bare_sub_headings_is_a_skeleton_not_an_entry(self):
        """`### Added` with nothing under it describes no release."""
        self.assertFalse(
            _describes_unreleased_work(
                changelog("## [Unreleased]\n\n### Added\n\n### Fixed\n"), "0.6.7"
            )
        )

    def test_a_changelog_with_no_unreleased_heading_at_all_is_not_an_entry(self):
        self.assertFalse(_describes_unreleased_work(PREAMBLE + "\n" + PUBLISHED, "0.6.7"))

    def test_prose_without_a_bullet_still_counts(self):
        """The check reads whether an entry exists, never whether it is good."""
        self.assertTrue(
            _describes_unreleased_work(
                changelog("## [Unreleased]\n\nEverything under the scan planner moved.\n"),
                "0.6.7",
            )
        )

    def test_blank_lines_alone_are_not_an_entry(self):
        self.assertFalse(_has_entries("\n   \n\t\n"))


class TestARolledChangelogIsAlreadyCorrect(unittest.TestCase):
    """The state a repo is in between rolling the changelog and bumping the
    version. `tins merge` refuses a release PR that touches anything but the
    version files, so the roll lands on main first and `[Unreleased]` sits
    empty until the bump. Firing here would fire on every well-run release."""

    def test_a_section_above_the_published_version_answers_for_the_release(self):
        text = changelog(
            "## [Unreleased]\n",
            "## [0.7.0] - 2026-09-07\n\n### Added\n- `TableScan.count()`.\n",
        )
        self.assertTrue(_describes_unreleased_work(text, "0.6.7"))
        self.assertIsNone(gap(text))

    def test_a_section_at_or_below_the_published_version_does_not(self):
        """0.6.7 is what is already on the registry; its own entry describes
        nothing that is owed."""
        self.assertFalse(_describes_unreleased_work(changelog("## [Unreleased]\n"), "0.6.7"))

    def test_an_empty_section_for_the_next_version_is_not_an_entry_either(self):
        text = changelog("## [Unreleased]\n", "## [0.7.0] - 2026-09-07\n")
        self.assertFalse(_describes_unreleased_work(text, "0.6.7"))

    def test_a_range_heading_is_read_without_tripping_over_it(self):
        """iceberg.mojo collapses a run of patch releases into one heading:
        `## [0.6.1] – [0.6.7] - …`. Both numbers are below the published one."""
        text = PREAMBLE + "\n## [Unreleased]\n\n## [0.6.1] – [0.6.7] - 2026-09-06\n\n- Several.\n"
        self.assertFalse(_describes_unreleased_work(text, "0.6.7"))

    def test_a_two_part_version_heading_is_ignored_rather_than_crashing(self):
        text = changelog("## [Unreleased]\n", "## [0.7] - 2026-09-07\n\n- Something.\n")
        self.assertFalse(_describes_unreleased_work(text, "0.6.7"))

    def test_an_unparsable_published_version_falls_back_to_unreleased_alone(self):
        text = changelog("## [Unreleased]\n\n- Something.\n")
        self.assertTrue(_describes_unreleased_work(text, "0.6.7-rc1"))
        self.assertFalse(
            _describes_unreleased_work(changelog("## [Unreleased]\n"), "0.6.7-rc1")
        )


class TestTheTwoCasesAreToldApart(unittest.TestCase):
    """A repo that has no changelog needs one created; a repo that has one
    needs the entry written. They are different jobs and the message says
    which."""

    def test_a_missing_file_says_to_start_one(self):
        message = gap(None)
        self.assertIsNotNone(message)
        self.assertIn("no CHANGELOG.md", message)
        self.assertIn("Keep a Changelog", message)

    def test_an_empty_unreleased_says_to_write_the_entry(self):
        message = gap(changelog("## [Unreleased]\n"))
        self.assertIsNotNone(message)
        self.assertIn("records nothing since 0.6.7", message)
        self.assertNotIn("no CHANGELOG.md", message)

    def test_a_written_entry_is_no_finding_at_all(self):
        self.assertIsNone(gap(changelog("## [Unreleased]\n\n### Added\n- A thing.\n")))


class TestItFiresOnlyWhereAReleaseIsOwed(unittest.TestCase):
    """The whole design constraint. Only 3 of ~22 magmalake repos have a
    CHANGELOG.md, and that is deliberate — one is started when a project next
    publishes, not backfilled. A finding on the other nineteen would be
    ignored, and a check that is ignored is worse than no check."""

    def _findings(self, repo_text, published_sha, main_sha):
        from tins import commands

        repo = mock.Mock(
            path="/tmp",
            ref=main_sha,
            slug="magmalake/x.mojo",
            version="0.6.7",
            tin="x-mojo",
            publishable=True,
            deps=[],
            other_checkouts=[],
            shelf={"name": "x-mojo", "version": "0.6.7"},
            version_files={"shelf.toml": "0.6.7"},
        )
        repo.dirty.return_value = False
        repo.head.return_value = main_sha
        registry = mock.Mock()
        registry.known.return_value = True
        registry.published.return_value = mock.Mock(sha=published_sha, version="0.6.7")
        args = mock.Mock(org=None, repo=None, tins=False, no_fetch=True)
        with (
            mock.patch.object(commands, "discover", return_value=[repo]),
            mock.patch.object(commands, "select", side_effect=lambda repos, *a, **k: repos),
            mock.patch.object(commands, "Registry", return_value=registry),
            mock.patch.object(commands, "_pin_states", return_value=[]),
            mock.patch.object(commands, "_changed_paths", return_value=["src/lib.mojo"]),
            mock.patch.object(commands, "_packaged_changes", return_value=["src/lib.mojo"]),
            mock.patch.object(commands, "read_at_ref", return_value=repo_text),
        ):
            _, findings = commands.diagnose(mock.Mock(registry="r"), args)
        return findings

    def test_a_stale_release_without_a_changelog_is_reported(self):
        codes = [f.code for f in self._findings(None, "aaa", "bbb")]
        self.assertEqual(codes, ["stale-release", "missing-changelog-entry"])

    def test_it_never_arrives_without_the_stale_release_it_hangs_off(self):
        """Same repo, published sha equal to main: no release owed, so no
        changelog owed either — whether or not the file exists."""
        codes = [f.code for f in self._findings(None, "aaa", "aaa")]
        self.assertNotIn("missing-changelog-entry", codes)
        self.assertNotIn("stale-release", codes)

    def test_it_is_a_warning_so_it_never_carries_the_exit_code_alone(self):
        """It only ever fires beside `stale-release`, which is already an
        error, so nothing is gained by making a prose check fail the build."""
        findings = self._findings(None, "aaa", "bbb")
        found = [f for f in findings if f.code == "missing-changelog-entry"]
        self.assertEqual([f.level for f in found], ["warn"])


class TestReleaseSaysItToo(unittest.TestCase):
    """`tins release` repeats the note beside each bump it plans. That is the
    last cheap moment: `merge` refuses a release PR that touches anything but
    the version files, so once the bump PR is open the entry costs a second
    one. It stays a note — the plan is still printed and `--yes` still works."""

    def _plan_output(self, changelog: str | None) -> str:
        import contextlib
        import io

        from argparse import Namespace

        from tins import commands

        repo = mock.Mock(
            path="/tmp", ref="bbb", slug="magmalake/x.mojo", version="0.6.7",
            tin="x-mojo", publishable=True,
        )
        registry = mock.Mock()
        registry.published.return_value = mock.Mock(sha="aaa", version="0.6.7")
        args = Namespace(org=None, repo=None, bump="minor", force=False, yes=False)
        buf = io.StringIO()
        with (
            mock.patch.object(commands, "discover", return_value=[repo]),
            mock.patch.object(commands, "select", side_effect=lambda repos, *a, **k: repos),
            mock.patch.object(commands, "topo_order", side_effect=lambda repos: repos),
            mock.patch.object(commands, "Registry", return_value=registry),
            mock.patch.object(commands, "_changed_paths", return_value=["src/lib.mojo"]),
            mock.patch.object(commands, "read_at_ref", return_value=changelog),
            contextlib.redirect_stdout(buf),
        ):
            commands.cmd_release(args, mock.Mock(registry="r"))
        return buf.getvalue()

    def test_a_bump_with_no_entry_is_flagged_in_the_plan(self):
        out = self._plan_output(changelog("## [Unreleased]\n"))
        self.assertIn("x-mojo  0.6.7 -> 0.7.0", out)
        self.assertIn("records nothing since 0.6.7", out)
        self.assertIn("re-run with --yes", out)

    def test_a_bump_with_an_entry_is_printed_plainly(self):
        out = self._plan_output(changelog("## [Unreleased]\n\n### Fixed\n- A thing.\n"))
        self.assertIn("x-mojo  0.6.7 -> 0.7.0", out)
        self.assertNotIn("CHANGELOG.md", out)


class TestThePlanSaysWhatToDo(unittest.TestCase):
    """There is no command that writes a changelog entry, and saying so beats
    silence: a reader who sees three of four findings addressed assumes the
    fourth was handled too."""

    def _notes(self) -> list[str]:
        from argparse import Namespace

        from tins import remedy
        from tins.commands import WARN, Finding

        repo = mock.Mock(
            org="magmalake", name="x.mojo", tin="x-mojo", deps=[], publishable=True
        )
        repo.slug = "magmalake/x.mojo"
        plan = remedy.build(
            [repo],
            [Finding("magmalake/x.mojo", WARN, "missing-changelog-entry", "")],
            Namespace(org=None, tins=False),
        )
        return plan.notes

    def test_the_finding_carries_advice(self):
        notes = self._notes()
        self.assertTrue(any("missing-changelog-entry (magmalake/x.mojo)" in n for n in notes))
        self.assertTrue(any("before the release step above" in n for n in notes))

    def test_it_is_not_left_in_the_catch_all(self):
        """`no command for:` is what remedy prints when somebody adds a
        finding and forgets to say what to do about it."""
        self.assertEqual([n for n in self._notes() if n.startswith("no command for")], [])


if __name__ == "__main__":
    unittest.main()
