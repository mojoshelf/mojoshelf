# mojoshelf

An experimental registry of reusable Mojo tins, installed as pixi source dependencies or git submodules.

See specs. Live at https://mojoshelf.org.

## Install the CLI

```sh
pixi global install --channel https://mojoshelf.org/channel mojoshelf
```

This exposes `shelf` and `pixi-shelf` (so `pixi shelf <cmd>` works). Without
pixi: `cargo install --locked --git https://github.com/mojoshelf/mojoshelf mojoshelf`.
The conda package is built for osx-arm64; new CLI versions are released with
`scripts/release-cli.sh`.

## Agent skills

Two [Agent Skills](https://agentskills.io) ship with this repo — one for
consuming tins, one for publishing them:

```sh
npx skills add mojoshelf/mojoshelf                                  # pick interactively
npx skills add mojoshelf/mojoshelf --skill mojoshelf-consume --yes  # or one directly
npx skills add mojoshelf/mojoshelf --skill mojoshelf-publish --yes
```

`skills/` is the source of truth for both. Working on them in this checkout,
run `scripts/link-skills.sh` instead of the commands above — it symlinks
`.claude/skills/<name>` at `skills/<name>`, so the agent loads the live file
and edits take effect immediately.

Do not run `npx skills add` on this repo from inside this repo. It is the
consumer-side command: it writes a *copy* of the skill under `.agents/`, plus
a `skills-lock.json` recording a content hash of that copy. In the authoring
repo the copy forks from `skills/` on the next edit and silently serves stale
instructions, and the lock hash is then a hash of the stale copy rather than
of anything you maintain. Both paths are gitignored.
