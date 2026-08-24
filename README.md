# mojoshelf

A experimental, git submodule based, registry of reusable Mojo modules.

See specs. Live at https://mojoshelf.org.

## Agent skills

Two [Agent Skills](https://agentskills.io) ship with this repo — one for
consuming books, one for publishing them:

```sh
npx skills add mojoshelf/mojoshelf                                  # pick interactively
npx skills add mojoshelf/mojoshelf --skill mojoshelf-consume --yes  # or one directly
npx skills add mojoshelf/mojoshelf --skill mojoshelf-publish --yes
```
