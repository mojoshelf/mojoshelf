# mojoshelf

A experimental, git submodule based, registry of reusable Mojo modules.

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
consuming books, one for publishing them:

```sh
npx skills add mojoshelf/mojoshelf                                  # pick interactively
npx skills add mojoshelf/mojoshelf --skill mojoshelf-consume --yes  # or one directly
npx skills add mojoshelf/mojoshelf --skill mojoshelf-publish --yes
```
