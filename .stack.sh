#!/usr/bin/env bash
# Stack a PR's own commits onto the integration branch.
# Usage: .stack.sh <pr-number>
set -uo pipefail
cd /Users/mfethe/buzz-integration
N="$1"
BASE=$(git merge-base upstream/main "pr/$N")
COMMITS=$(git rev-list --reverse "$BASE".."pr/$N")
NC=$(echo "$COMMITS" | grep -c . || true)

# patch-ids already on upstream/main (superseded detection)
UPSTREAM_IDS=$(git log --since="21 days ago" -p upstream/main 2>/dev/null | git patch-id --stable 2>/dev/null | awk '{print $1}' | sort -u)

applied=0; skipped=0; failed=0
for c in $COMMITS; do
  pid=$(git show "$c" | git patch-id --stable | awk '{print $1}')
  if [ -n "$pid" ] && echo "$UPSTREAM_IDS" | grep -q "^$pid$"; then
    echo "SKIP(upstream-dupe) $(git log -1 --format='%h %s' $c)"
    skipped=$((skipped+1)); continue
  fi
  if git cherry-pick -x --signoff "$c" >/dev/null 2>&1; then
    applied=$((applied+1))
  else
    if git diff --name-only --diff-filter=U | grep -q .; then
      echo "CONFLICT $(git log -1 --format='%h %s' $c)"
      git diff --name-only --diff-filter=U | sed 's/^/    /'
      git cherry-pick --abort 2>/dev/null
      failed=$((failed+1))
    else
      # empty after dedupe = already present
      git cherry-pick --skip 2>/dev/null || git cherry-pick --abort 2>/dev/null
      echo "SKIP(empty) $(git log -1 --format='%h %s' $c)"
      skipped=$((skipped+1))
    fi
  fi
done
echo "PR#$N total=$NC applied=$applied skipped=$skipped conflicted=$failed"
