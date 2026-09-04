"""tins — release plumbing for a polyrepo of mojoshelf tins."""

from __future__ import annotations

import argparse
import sys

from . import commands
from .config import Config
from .gitutil import CommandError
from .registry import RegistryError


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="tins", description=__doc__)
    p.add_argument("--config", help="path to tins.toml (default: the one in this repo)")
    p.add_argument("--org", help="only repos whose GitHub org is this")
    p.add_argument("--repo", action="append", help="only this repo (repeatable; name or tin)")
    p.add_argument("--tins", action="store_true", help="only repos that publish a tin")
    p.add_argument(
        "--no-fetch",
        action="store_true",
        help="read the working tree instead of fetching and reading origin/main",
    )
    p.add_argument("-v", "--verbose", action="store_true", help="include informational findings")
    sub = p.add_subparsers(dest="command_name", required=True)

    lst = sub.add_parser("list", help="one line per repo: version, published version, pin state")
    lst.set_defaults(fn=commands.cmd_list)

    graph = sub.add_parser("graph", help="print the tins in publish order")
    graph.set_defaults(fn=commands.cmd_graph)

    doctor = sub.add_parser("doctor", help="check versions, pins and registry state")
    doctor.set_defaults(fn=commands.cmd_doctor)

    sweep = sub.add_parser(
        "sweep",
        help="run a command in a worktree of every repo, then commit, push and open a PR",
    )
    sweep.add_argument("--branch", required=True, help="branch to create in each repo")
    sweep.add_argument("--title", required=True, help="commit subject and PR title")
    sweep.add_argument("--body", help="commit body and PR body")
    sweep.add_argument("--body-file", help="read the body from this file")
    sweep.add_argument("--script", help="run this script instead of a -- command")
    sweep.add_argument("--lock", action="store_true", help="run pixi lock before committing")
    sweep.add_argument("--no-pr", dest="pr", action="store_false", help="push without opening a PR")
    sweep.add_argument("--dry-run", action="store_true")
    sweep.add_argument("command", nargs=argparse.REMAINDER, help="-- command to run in each worktree")
    sweep.set_defaults(fn=commands.cmd_sweep)

    publish = sub.add_parser("publish", help="shelf publish every tin whose version is not on the registry")
    publish.add_argument("--yes", action="store_true", help="actually publish (default is a plan)")
    publish.add_argument("--force", action="store_true", help="publish even with stale package pins")
    publish.set_defaults(fn=commands.cmd_publish)

    release = sub.add_parser(
        "release", help="open a version-bump PR for every tin whose published rev is behind main"
    )
    release.add_argument("--yes", action="store_true", help="actually open them (default is a plan)")
    release.add_argument("--bump", default="patch", choices=["patch", "minor", "major"])
    release.add_argument("--branch", help="branch name (default: release-<new version>)")
    release.add_argument("--title", help="commit subject and PR title (default: Release <new version>)")
    release.add_argument(
        "--force",
        action="store_true",
        help="include repos where only CI or docs changed since the published rev",
    )
    release.add_argument("--no-pr", dest="pr", action="store_false", help="push without opening a PR")
    release.set_defaults(fn=commands.cmd_release)

    merge = sub.add_parser(
        "merge", help="merge open PRs that are provably nothing but a version bump"
    )
    merge.add_argument("--yes", action="store_true", help="actually merge (default is a plan)")
    merge.add_argument(
        "--merge-method",
        default="squash",
        choices=["squash", "merge", "rebase"],
        help="how to merge (default: squash, matching how this org merges)",
    )
    merge.add_argument(
        "--allow-no-checks",
        action="store_true",
        help="merge a PR whose head revision reports no checks at all "
        "(a repo without CI — but also a PR whose workflows have not started yet)",
    )
    merge.set_defaults(fn=commands.cmd_merge)

    repin = sub.add_parser("repin", help="move stale pins to the published revs and open PRs")
    repin.add_argument("--branch", default="repin", help="branch to create (default: repin)")
    repin.add_argument("--title", help="commit subject and PR title")
    repin.add_argument("--bump", default="patch", choices=["patch", "minor", "major"])
    repin.add_argument(
        "--package-only",
        action="store_true",
        help="only pins under [package.*-dependencies]",
    )
    repin.add_argument(
        "--unpublished-only",
        action="store_true",
        help="only pins that name a revision the registry never published",
    )
    repin.add_argument("--no-pr", dest="pr", action="store_false", help="push without opening a PR")
    repin.add_argument("--dry-run", action="store_true")
    repin.set_defaults(fn=commands.cmd_repin)

    return p


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if getattr(args, "command", None) and args.command and args.command[0] == "--":
        args.command = args.command[1:]
    config = Config.load(args.config)
    try:
        return args.fn(args, config)
    except CommandError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    except RegistryError as e:
        print(f"error: cannot reach the registry: {e}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
