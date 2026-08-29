#!/bin/bash
#
# Link this repo's skills into the local agent skill directory, so an agent
# working in this checkout loads them straight from skills/.
#
# Do NOT run `npx skills add mojoshelf/mojoshelf` inside this repo. That is the
# consumer-side command: it writes a *copy* of the skill under .agents/, a
# symlink under .claude/skills/, and a skills-lock.json pinning a content hash.
# In the authoring repo the copy immediately forks from skills/ and goes stale,
# and the lock records a hash of the stale copy. Both are gitignored here for
# that reason.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

mkdir -p .claude/skills
for dir in skills/*/; do
  name="$(basename "$dir")"
  [ -f "$dir/SKILL.md" ] || { echo "skipping $name (no SKILL.md)"; continue; }
  ln -sfn "../../skills/$name" ".claude/skills/$name"
  echo "linked .claude/skills/$name -> skills/$name"
done
