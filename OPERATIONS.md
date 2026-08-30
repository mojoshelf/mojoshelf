# Operations

Running the mojoshelf Worker: secrets, deploys, migrations, and what to check
when the registry stops updating itself.

Everything here runs from `crates/shelf-worker` unless stated otherwise, and
assumes `wrangler` is authenticated for the account that owns `mojoshelf.org`.

## GITHUB_TOKEN

The sync cron calls the GitHub API to refresh each tin's stars, forks and
commit counts, which feed the interestingness score the tin list is ranked by.
Without a working token the refresh silently falls behind: a Worker has no
dedicated egress IP, so unauthenticated calls share one address with every
other tenant on it and the anonymous 60/hr budget is generally already spent.

### Generate the token

**Fine-grained** (github.com/settings/personal-access-tokens/new):

- **Expiration: 366 days or less.** Some orgs refuse longer-lived
  fine-grained tokens by policy and answer `403` to every request, including
  ones for their public repos. `labelrefinery` is one of them.
- Resource owner: your own account.
- Repository access: **Public repositories** — the registry only ever reads
  public metadata.
- Permissions: none beyond the read-only public access that setting implies.

**Classic** (github.com/settings/tokens/new) also works: tick `public_repo`
and nothing else. Classic tokens are not subject to the fine-grained lifetime
policies, at the cost of being coarser.

### Save it to the Worker

```sh
cd crates/shelf-worker
npx wrangler secret put GITHUB_TOKEN
# paste the token at the prompt; it is never echoed or stored in the repo
```

The secret takes effect immediately — no redeploy needed. Confirm it landed:

```sh
npx wrangler secret list
```

That lists names only, never values. To confirm the token actually *works*,
trigger a sync (below) and check that `liveliness` reports a count rather than
an error.

### The other secrets

| secret | used for |
|---|---|
| `GITHUB_TOKEN` | GitHub API reads for the liveliness refresh and cards |
| `GITHUB_CLIENT_SECRET` | GitHub OAuth sign-in on `/authors` |
| `SESSION_SECRET` | signing author session cookies |

The PostHog project key is not a secret and not a var: it is
`html::POSTHOG_KEY`, a constant in the source, because it is a public
client-side key that ships in the HTML of every page anyway. Changing it means
editing `crates/shelf-worker/src/html.rs` and redeploying. Setting it to
anything not beginning with `phc_` omits the browser snippet entirely.

## Deploying

```sh
rustup target add wasm32-unknown-unknown   # once
cargo install worker-build                 # once; wrangler shells out to it
cd crates/shelf-worker && npx wrangler deploy
```

`npx wrangler deploy` publishes whatever is in your working tree, so commit
first if you want the deployed Worker to match a commit. CI only deploys on
`cli-v*` tags, so pushing to `main` does not deploy.

### Migrations come first

`wrangler deploy` does **not** run D1 migrations. Deploying code that selects a
column the database does not have yet takes the whole site down, so when a
change adds a migration:

```sh
npx wrangler d1 migrations list mojoshelf --remote    # what is pending
npx wrangler d1 migrations apply mojoshelf --remote   # apply it
npx wrangler deploy                                   # then deploy
```

Rolling back is the reverse: deploy the older Worker first, then reverse the
migration by hand, since `migrations apply` only moves forwards.

## The sync cron

`crons: ["23 */6 * * *"]` — 00:23, 06:23, 12:23 and 18:23 UTC. Each run
mirrors the modular-community channel, enriches channel tins, refreshes
liveliness for `LIVELINESS_BATCH` (10) repos and rebuilds `CARD_BATCH` (4)
agent cards. The batches are small to stay inside the Workers subrequest cap,
so a full pass over the registry takes a day or so.

Trigger one by hand with any valid publish token — it is idempotent, and the
response carries the per-phase result, which is the fastest way to see what is
wrong:

```sh
curl -s -X POST https://mojoshelf.org/api/sync-channel \
  -H "Authorization: Bearer $SHELF_TOKEN"
```

```json
{"ok":true,"result":"mirrored 43 channel packages, pruned 0, enriched 0, liveliness 10, cards 4"}
```

A phase that fails reports `liveliness ERROR: …` there and files a
`SyncPhaseError` in PostHog Error Tracking, tagged with the phase. The other
phases still run: one broken phase does not stop the sync.

## When scores or activity stop updating

The tin list ranks by a cached `score`, computed from stars, forks and commit
counts when liveliness is refreshed. Tins never refreshed have no score and
sort last, and the list heading falls back to plain "Tins" rather than
claiming a ranking it does not have.

Check how stale the data is:

```sh
npx wrangler d1 execute mojoshelf --remote --command \
  "SELECT COUNT(*) AS total, SUM(liveliness_at IS NULL) AS never_refreshed, \
          SUM(score IS NOT NULL) AS scored, MAX(liveliness_at) AS newest FROM tins"
```

`newest` should be within a few hours. If it is days old, run a manual sync and
read the `liveliness` field:

- **`github refused the token for …: 403: {"message":"The '<org>' organization
  forbids access via a fine-grained personal access tokens if the token's
  lifetime is greater than 366 days…"}`** — reissue `GITHUB_TOKEN` with a
  shorter lifetime, per the section above.
- **`403: {"message":"API rate limit exceeded for <ip>"}`** on the anonymous
  retry — the token was refused and the unauthenticated fallback is throttled
  on the shared Worker egress IP. Same fix: a token the org accepts.
- **`all N repos failed`** — nothing in the batch was readable, which is a
  configuration problem rather than a quiet day.
- **a count, but scores stay 0** — the refresh is working and simply has not
  reached every tin yet, at 10 per run. Run the sync repeatedly to hurry it.

Repos that cannot be read are stamped anyway so they move to the back of the
queue. Without that, unreadable repos keep a NULL `liveliness_at`, the stale
queue puts NULL first, and they sit at the head of every batch starving every
other tin — which is what froze the refresh for two days in August 2026.

## Error tracking

Worker errors go to PostHog as `$exception` events: `WorkerError` for a failed
request (with the route), `SyncPhaseError` for a sync phase, `ChannelSyncError`
for a cron that fails outright. There is no stack trace — a wasm Worker has no
unwinder — so `Located::at()` records the file and line of the `?` that failed
and reports it as the exception's single frame.

Live logs, when a run needs watching as it happens:

```sh
npx wrangler tail
```
