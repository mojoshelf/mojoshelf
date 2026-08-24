#!/bin/bash
#
# Release a new version of the shelf CLI to the conda channel served at
# https://mojoshelf.org/channel.
#
# Before running: bump the version in crates/mojoshelf/Cargo.toml,
# pixi.toml ([package]), and recipe.yaml — all three must match.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHANNEL="$ROOT/crates/shelf-worker/public/channel"

cd "$ROOT"
pixi build
mv mojoshelf-*.conda "$CHANNEL/osx-arm64/"
pixi exec rattler-index fs "$CHANNEL"

echo "Channel updated. Deploy it with:"
echo "  cd crates/shelf-worker && npx wrangler deploy"
echo "Then commit the new package and repodata."
