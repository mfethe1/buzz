#!/usr/bin/env bash
#
# absorb-renumber-migrations.sh — resolve duplicate migration version prefixes
# created by absorbing upstream/main into product/main.
#
# WHY THIS EXISTS
#   sqlx 0.9 has no duplicate-migration-version error class, and `migrate!`
#   neither dedups nor rejects. Two files sharing a version prefix are embedded
#   as two entries, so the failure surfaces at relay startup inside
#   run_migrations — while it holds SCHEMA_DESTRUCTION_LOCK_KEY on a
#   half-migrated schema. A clean `git merge` is NOT proof of correctness: the
#   merge succeeds precisely because the colliding filenames differ.
#
# DIRECTION OF THE FIX (precedent a970fda8f2, 2026-09-20)
#   The INCOMING upstream file is renamed, never the fork file. Fork migrations
#   have already been applied to running relays; renaming one would either
#   re-run it or strand its recorded checksum. Upstream's are not yet applied
#   here, so the incoming side moves.
#
# CONTRACT
#   Run INSIDE a conflicted-or-clean absorb merge (MERGE_HEAD present), from the
#   repo root. Renames are `git mv`, so content blobs are byte-identical; the
#   script verifies that itself and aborts if any blob SHA changed.
#
# USAGE
#   git merge --no-commit --no-ff upstream/main   # resolve other conflicts too
#   scripts/absorb-renumber-migrations.sh          # then commit the merge
#
#   --dry-run   print the plan, change nothing
#
set -euo pipefail

DRY_RUN=0
[ "${1:-}" = "--dry-run" ] && DRY_RUN=1

cd "$(git rev-parse --show-toplevel)"

if [ ! -e "$(git rev-parse --git-dir)/MERGE_HEAD" ]; then
  echo "error: no merge in progress; run this inside the absorb merge." >&2
  exit 2
fi

OURS=$(git rev-parse HEAD)
THEIRS=$(git rev-parse MERGE_HEAD)

# Version prefixes that appear more than once in the merged index.
# Kept as a newline-delimited string rather than an array: empty arrays under
# `set -u` are an unbound-variable error in the /bin/bash 3.2 that ships on
# macOS, and `mapfile` does not exist there at all.
DUPS=$(
  git ls-files migrations/ \
    | sed 's|migrations/||' \
    | cut -d_ -f1 \
    | sort | uniq -d
)

if [ -z "$DUPS" ]; then
  echo "no duplicate migration prefixes in the merged tree; nothing to do."
  exit 0
fi

# Every prefix currently occupied, in EITHER parent as well as the merged
# index. A slot freed by this very rename must not be handed back out, and a
# slot occupied only on one side is still spoken for.
occupied() {
  {
    git ls-files migrations/
    git ls-tree --name-only "$OURS"   migrations/
    git ls-tree --name-only "$THEIRS" migrations/
  } | sed 's|migrations/||' | cut -d_ -f1 | sort -u
}

OCCUPIED=$(occupied)

# Slots claimed by an OPEN upstream pull request. Losing a race with a
# contributor who already owns a number reintroduces the collision one merge
# later, so those numbers are skipped when the data is available. Absence of
# `gh` is not fatal — it only widens the candidate set.
CLAIMED=""
if command -v gh >/dev/null 2>&1; then
  CLAIMED=$(
    gh pr list --repo block/buzz --state open --limit 600 \
      --json files --jq '.[].files[].path' 2>/dev/null \
      | sed -n 's|^migrations/\([0-9]\{4\}\)_.*|\1|p' \
      | sort -u
  ) || CLAIMED=""
fi

next_free() {
  local n=$1
  while :; do
    local slot
    slot=$(printf '%04d' "$n")
    if ! grep -qx "$slot" <<<"$OCCUPIED" && ! grep -qx "$slot" <<<"${CLAIMED:-}"; then
      printf '%s' "$slot"
      return
    fi
    n=$((n + 1))
  done
}

TAIL=$(printf '%s\n' "$OCCUPIED" | sort | tail -1)
CURSOR=$((10#$TAIL + 1))
RC=0

for prefix in $DUPS; do
  # The incoming file is the one present in MERGE_HEAD but not in HEAD.
  incoming=""
  while IFS= read -r path; do
    name=${path#migrations/}
    [ "${name%%_*}" = "$prefix" ] || continue
    if ! git cat-file -e "$OURS:$path" 2>/dev/null; then
      incoming=$name
    fi
  done < <(git ls-tree --name-only "$THEIRS" migrations/)

  if [ -z "$incoming" ]; then
    echo "SKIP  $prefix: no unambiguous incoming file (both sides fork-local?)" >&2
    RC=1
    continue
  fi

  slot=$(next_free "$CURSOR")
  CURSOR=$((10#$slot + 1))
  OCCUPIED=$(printf '%s\n%s\n' "$OCCUPIED" "$slot" | sort -u)

  target="${slot}_${incoming#*_}"
  before=$(git rev-parse "$THEIRS:migrations/$incoming")

  echo "RENAME $incoming -> $target   (blob $before)"
  [ "$DRY_RUN" -eq 1 ] && continue

  git mv "migrations/$incoming" "migrations/$target"

  after=$(git hash-object "migrations/$target")
  if [ "$before" != "$after" ]; then
    echo "error: content changed during rename of $incoming ($before -> $after)" >&2
    exit 3
  fi

  # Source references the file by literal name (include_str!, docs). Leaving one
  # dangling turns a rename into a compile error at a later, unrelated commit.
  while IFS= read -r ref; do
    [ -n "$ref" ] || continue
    echo "  ref   $ref"
    perl -pi -e "s/\Q$incoming\E/$target/g" "$ref"
  done < <(
    git grep -l --fixed-strings "$incoming" -- \
      ':!migrations/' ':!*.lock' 2>/dev/null || true
  )
done

[ "$DRY_RUN" -eq 1 ] && exit 0

remaining=$(git ls-files migrations/ | sed 's|migrations/||' | cut -d_ -f1 | sort | uniq -d)
if [ -n "$remaining" ]; then
  echo "error: duplicate prefixes remain after renumber:" >&2
  printf '%s\n' "$remaining" >&2
  exit 4
fi

echo "OK: no duplicate migration prefixes in the merged tree."
exit "$RC"
