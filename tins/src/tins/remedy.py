"""Turn doctor's findings into the commands that clear them.

`doctor` names problems. This names the fix, and the order it prints them in
carries most of the value: a repo that owes a release must publish *before*
its consumers are re-pinned, because the re-pin then folds into the
consumer's own next release. Do it the other way round and every consumer
pays for two releases — one to publish whatever it already has, another to
move the pin once the dependency finally lands.

Nothing here runs anything. It reads the findings `doctor` already produced
and prints a plan the reader can copy, edit or ignore.
"""

from __future__ import annotations

from dataclasses import dataclass, field

# Findings with no command behind them. Saying so plainly beats suggesting
# something vague, and beats silence — a reader who sees four of five
# findings addressed will assume the fifth was handled too.
NO_COMMAND = {
    "missing-changelog-entry": (
        "write the entry by hand and land it on main before the release step above — "
        "`tins merge` refuses a release PR that touches anything but the version files, "
        "so the changelog cannot ride along in the bump"
    ),
    "shelf-tins-drift": "add the missing pins to shelf.toml's `tins` list by hand",
    "unregistered": "publish once by hand, or drop it from the workspace",
    "dirty": "commit or discard the local changes",
    "behind": "git pull in the checkout",
    "duplicate-checkout": "delete the clone you are not using",
}

# Findings that are already someone else's step, or are purely informational.
_SUBSUMED = {"unreleased-commits", "unregistered-dep"}

# How each subcommand is told to act. Most default to printing a plan and
# need `--yes`; `repin` acts by default and takes `--dry-run` instead. The
# round-trip test in tests/test_remedy.py is what keeps this honest -- a
# printed command that the CLI rejects is worse than no suggestion.
_CONFIRM = {"fix": "--yes", "release": "--yes", "merge": "--yes", "publish": "--yes", "repin": ""}


@dataclass
class Command:
    """A command in the plan, in the form that both prints and runs.

    `text` is what the reader sees and `argv` is what `tins run` executes,
    built from the same fields so the two cannot drift. A guided run that
    quietly does something other than what it displayed would be worse than
    no guidance at all.
    """

    sub: str
    slugs: list[str]
    text: str
    argv: list[str]

    @property
    def opens_prs(self) -> bool:
        return self.sub in ("fix", "release", "repin")


@dataclass
class Step:
    """One line of the plan: a command, why it is there, and its position."""

    commands: list[Command]
    why: str


@dataclass
class Plan:
    steps: list[Step] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)

    def __bool__(self) -> bool:
        return bool(self.steps or self.notes)


def _scope(slugs: list[str], repos_by_slug: dict, args) -> str:
    """Flags naming exactly `slugs`, or the caller's own scope if that is shorter.

    Naming repos explicitly is what makes a suggested command safe to paste:
    it cannot act on a repo the reader did not just read a finding about.
    Past four it stops being readable, and the scope the reader already
    typed is the better handle.

    These are global flags, so they belong before the subcommand -- `tins
    --repo x release`, not `tins release --repo x`, which argparse rejects.
    """
    if len(slugs) <= 4:
        names = [repos_by_slug[s].name for s in slugs]
        return " ".join(f"--repo {n}" for n in sorted(names))
    parts = []
    if getattr(args, "org", None):
        parts.append(f"--org {args.org}")
    if getattr(args, "tins", False):
        parts.append("--tins")
    return " ".join(parts)


def _consumers(repos, tins: set[str]) -> set[str]:
    """Slugs of repos that pin any tin in `tins`, transitively."""
    by_tin = {r.tin: r for r in repos if r.tin}
    reached: set[str] = set()
    frontier = set(tins)
    while frontier:
        nxt: set[str] = set()
        for r in repos:
            if r.slug in reached:
                continue
            deps = {d.pkg for d in r.deps if d.is_package_dep}
            if deps & frontier:
                reached.add(r.slug)
                if r.tin:
                    nxt.add(r.tin)
        frontier = nxt & set(by_tin)
    return reached


def build(repos, findings, args) -> Plan:
    """The plan for `findings`, in the order they have to be worked.

    Only findings the reader was actually shown reach here — `doctor` hides
    INFO without `-v`, and advice for a finding nobody saw reads as noise
    attached to nothing.
    """
    by_slug = {r.slug: r for r in repos}
    kinds: dict[str, list[str]] = {}
    for f in findings:
        if f.repo in by_slug:
            seen = kinds.setdefault(f.code, [])
            if f.repo not in seen:
                seen.append(f.repo)

    plan = Plan()

    def cmd(sub: str, slugs: list[str], *extra: str) -> Command:
        """`tins <scope> <subcommand> <flags>` -- scope first, always."""
        scope = _scope(slugs, by_slug, args)
        argv = [*scope.split(), sub, *extra]
        if confirm := _CONFIRM[sub]:
            argv.append(confirm)
        return Command(sub, list(slugs), " ".join(["tins", *argv]), argv)

    # 1. Versions that disagree with themselves, first. A release bumps from
    #    the version it reads, so bumping a repo whose files disagree just
    #    moves the disagreement to a new pair of numbers.
    if mismatch := kinds.get("version-mismatch"):
        plan.steps.append(
            Step(
                [cmd("fix", mismatch)],
                f"{len(mismatch)} repo(s) whose version files disagree — do this first, "
                f"so anything that bumps a version starts from the right one",
            )
        )

    stale = kinds.get("stale-release", [])
    stale_tins = {by_slug[s].tin for s in stale if by_slug[s].tin}

    # 2. Work that is merged but reaches nobody. Each of these owes a version
    #    bump, a merge and a publish — three commands, one dependency chain.
    if stale:
        plan.steps.append(
            Step(
                [
                    cmd("release", stale, "--bump", "minor"),
                    cmd("merge", stale),
                    cmd("publish", stale),
                ],
                f"{len(stale)} repo(s) with merged src/ changes that reach nobody until "
                f"a bump is published",
            )
        )

    # 3. The optimisation. A repo whose version is bumped but unpublished, and
    #    which consumes something in step 2, should not publish yet: its pin is
    #    about to go stale, and the re-pin would then owe it a second release.
    #    Held here, the pin move folds into the release it has not made yet.
    unpub = kinds.get("unpublished", [])
    # Anything about to reach the registry with a new version invalidates
    # its consumers' pins -- whether it owes a release (step 2) or is
    # already bumped and merely unpublished. The second case is the same
    # situation one step later, and missing it costs the same extra
    # release: the consumer publishes, then has to re-pin and publish again.
    publishing = stale_tins | {by_slug[s].tin for s in unpub if by_slug[s].tin}
    downstream = _consumers(repos, publishing) if publishing else set()
    held = [s for s in unpub if s in downstream]
    publish_now = [s for s in unpub if s not in downstream]

    if publish_now:
        plan.steps.append(
            Step(
                [cmd("publish", publish_now)],
                f"{len(publish_now)} repo(s) already bumped, not on the registry",
            )
        )

    # 4. Pins that need moving — the stale ones doctor found, plus anything
    #    held back above, whose pin the release step has just invalidated.
    pins = kinds.get("unpublished-pin", []) + kinds.get("outdated-pin", [])
    repin = sorted(set(pins) | set(held))
    if repin:
        # A held repo's pin is not stale yet -- step 2 is what makes it
        # stale -- so it needs its own sentence rather than being folded
        # into a count of pins that are already wrong.
        parts = []
        if pins:
            parts.append(f"{len(set(pins))} repo(s) pinning a rev that is not the published one")
        if held:
            names = ", ".join(sorted(by_slug[s].name for s in held))
            parts.append(
                f"{names} goes stale the moment the step above publishes, and is held until now "
                f"on purpose — its own unpublished version absorbs the pin move, which saves it "
                f"a second release"
            )
        why = "; ".join(parts)
        cmds = [cmd("repin", repin), cmd("merge", repin)]
        # Only a repo that publishes a tin has anything to publish. A
        # consumer that is nobody's dependency -- an app, an example -- takes
        # the pin and stops there, and naming it in a publish would be a
        # command that fails.
        if tins := [s for s in repin if by_slug[s].publishable and by_slug[s].tin]:
            cmds.append(cmd("publish", tins))
        plan.steps.append(Step(cmds, why))

    for code, advice in NO_COMMAND.items():
        if slugs := kinds.get(code):
            plan.notes.append(f"{code} ({', '.join(sorted(slugs))}): no command — {advice}")

    handled = (
        {"version-mismatch", "stale-release", "unpublished", "unpublished-pin", "outdated-pin"}
        | set(NO_COMMAND)
        | _SUBSUMED
    )
    if unknown := sorted(set(kinds) - handled):
        plan.notes.append(f"no command for: {', '.join(unknown)}")

    return plan


def render(plan: Plan) -> None:
    if not plan:
        return
    print("\nnext steps")
    for i, step in enumerate(plan.steps, 1):
        for j, c in enumerate(step.commands):
            print(f"  {str(i) + '.' if j == 0 else '  '} {c.text}")
        print(f"     {step.why}")
    for note in plan.notes:
        print(f"  · {note}")
    if plan.steps:
        print("\n  each step waits on the one above it")
