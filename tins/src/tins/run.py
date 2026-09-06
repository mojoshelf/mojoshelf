"""The guided loop: one step at a time, approved from the terminal.

`doctor` prints a plan and leaves the reader to paste commands and open a
browser to approve each PR. That round trip is most of the work, and it is
where the plan's ordering gets lost -- a browser tab does not know that
iceberg is meant to wait.

So this walks the plan instead. For each step it shows why the step exists,
then the command, then asks. After a command that opens pull requests it
prints the diffs inline -- the same file patches `tins merge` judges on, so
what the reader approves is what the validator saw -- and asks again.

Two properties matter more than convenience:

*   **Nothing runs unapproved.** Every command is a separate keystroke, and
    a non-tty gets a refusal rather than a default of yes.
*   **What runs is what was printed.** Commands are re-parsed from their own
    displayed text through the real parser, so a step cannot execute
    something other than what the reader read.

The plan is rebuilt after every step. That is not caution, it is the point:
each step changes what the next one should be, and a plan computed once at
the start would be describing a workspace that no longer exists.
"""

from __future__ import annotations

import sys

from . import gitutil, remedy
from .commands import INFO, _print_findings, diagnose
from .config import Config

_QUIT = object()

# Enough of a bump diff to see it whole; past that something is wrong with
# the premise that these are three-line changes, and the reader should look
# at the PR itself rather than scroll a terminal.
_DIFF_LINES = 120


def _ask(question: str, choices: dict[str, str], default: str) -> str:
    """One keystroke, echoed back. Returns a choice key, or `_QUIT`."""
    menu = " ".join(f"[{k}]{v}" for k, v in choices.items())
    while True:
        try:
            raw = input(f"\n{question}\n  {menu} ({default}) > ").strip().lower()
        except (EOFError, KeyboardInterrupt):
            print()
            return "q"
        if not raw:
            return default
        if raw in choices:
            return raw
        for k in choices:
            if raw == choices[k].lower():
                return k
        print(f"  not one of: {', '.join(choices)}")


def _show_diffs(slugs: list[str], repos) -> None:
    """The open PRs of the repos a command just touched, patches and all."""
    by_slug = {r.slug: r for r in repos}
    for slug in slugs:
        r = by_slug.get(slug)
        if r is None:
            continue
        prs = gitutil.open_prs(r.org, r.name)
        if prs is None:
            print(f"\n! {slug}: could not list its pull requests")
            continue
        for pr in prs:
            print(f"\n─── {slug} #{pr['number']}  {pr['title']}")
            print(f"    {pr['url']}")
            files = gitutil.pr_files(r.org, r.name, pr["number"])
            if files is None:
                print("    (could not read the diff — review it in the browser)")
                continue
            shown = 0
            for f in files:
                print(f"    {f.get('filename')}")
                for line in (f.get("patch") or "").splitlines():
                    if shown >= _DIFF_LINES:
                        break
                    if line.startswith(("+", "-")) and not line.startswith(("+++", "---")):
                        print(f"      {line}")
                        shown += 1
            if shown >= _DIFF_LINES:
                print(f"      … truncated at {_DIFF_LINES} changed lines")


def cmd_run(args, config: Config) -> int:
    if not sys.stdin.isatty():
        print(
            "error: `tins run` is interactive and stdin is not a terminal.\n"
            "       Use `tins doctor` for the plan, then run its steps yourself.",
            file=sys.stderr,
        )
        return 2

    from .__main__ import build_parser

    done = 0
    while True:
        repos, findings = diagnose(config, args)
        shown = [f for f in findings if args.verbose or f.level != INFO]
        plan = remedy.build(repos, shown, args)

        if not plan.steps:
            if done:
                print(f"\nnothing left to do — {done} step(s) completed")
            else:
                print("\nnothing to do")
            for note in plan.notes:
                print(f"  · {note}")
            return 0

        step = plan.steps[0]
        remaining = len(plan.steps)
        print(f"\n{'━' * 72}\nstep 1 of {remaining}{'' if remaining == 1 else ' remaining'}")
        print(f"\n  {step.why}\n")
        for c in step.commands:
            print(f"    {c.text}")

        choice = _ask(
            "run this step?",
            {"y": "yes", "s": "skip", "f": "findings", "q": "quit"},
            "y",
        )
        if choice == "q":
            print(f"\nstopped — {done} step(s) completed, the rest is still in `tins doctor`")
            return 0
        if choice == "f":
            _print_findings(shown, args.verbose)
            continue
        if choice == "s":
            # Skipping the first step usually strands the rest, since the
            # plan's order is a dependency order. Saying so is more useful
            # than silently working a plan whose premise just changed.
            print("\nskipped — later steps may depend on it; re-check with `tins doctor`")
            return 0

        for c in step.commands:
            print(f"\n$ {c.text}")
            sub = build_parser().parse_args(c.argv)
            sub.config = args.config
            code = sub.fn(sub, config)
            if code not in (0, None):
                print(f"\n`{c.text}` exited {code} — stopping here rather than guessing")
                return code

            if c.opens_prs:
                _show_diffs(c.slugs, repos)
                nxt = _ask(
                    "the next command merges these. continue?",
                    {"y": "yes", "q": "stop here"},
                    "y",
                )
                if nxt == "q":
                    print("\nstopped — the pull requests are open and unmerged")
                    return 0

        done += 1
