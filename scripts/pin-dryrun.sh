#!/bin/sh
# Runs the publish-time pin check over the manifests of tins already published,
# without publishing anything: fetches each named tin's record from the
# registry and its pixi manifest at the latest version's commit, then runs the
# ignored `dry_run_over_published_manifests` test over them.
#
#   scripts/pin-dryrun.sh [tin ...]     (default: iceberg-mojo orca-mojo parquet-mojo)
#
# Every tin the named ones depend on is fetched too, so the check sees the
# registry state it would at publish time. Set SHELF_REGISTRY to use another.
set -eu
reg=${SHELF_REGISTRY:-https://mojoshelf.org}
[ $# -gt 0 ] || set -- iceberg-mojo orca-mojo parquet-mojo
dir=$(mktemp -d "${TMPDIR:-/tmp}/pin-dryrun.XXXXXX")
mkdir "$dir/tins" "$dir/manifests"
all=$(curl -fsS "$reg/api/tins" | python3 -c 'import json,sys; print(*[t["name"] for t in json.load(sys.stdin) if t["kind"]=="source"])')
for t in $all; do
  curl -fsS "$reg/api/tins/$t" > "$dir/tins/$t.json"
done
for t in "$@"; do
  python3 - "$dir" "$t" <<'PY' | while read -r url sha sub; do
import json, sys
d = json.load(open(f"{sys.argv[1]}/tins/{sys.argv[2]}.json"))
v = d["versions"][0]
print(d["url"], v["commit_sha"], d.get("subdirectory") or "-")
PY
    slug=${url#https://github.com/}; slug=${slug%.git}
    [ "$sub" = "-" ] && sub="" || sub="$sub/"
    curl -fsS "https://raw.githubusercontent.com/$slug/$sha/${sub}pixi.toml" > "$dir/manifests/$t.toml"
  done
done
PIN_DRYRUN_DIR=$dir cargo test -p shelf-worker dry_run_over_published_manifests -- --ignored --nocapture
rm -rf "$dir"
