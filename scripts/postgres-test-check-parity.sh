#!/usr/bin/env bash
# Compare the actual test bootstrap with PostgreSQL's direct interpretation of
# the desired schema. In particular, pgschema must not silently omit CHECKs.
set -euo pipefail

: "${BUZZ_POSTGRES_ADMIN_URL:?set an administrator database URL for isolated test databases}"
: "${PGDATABASE:?set the existing desired-state test database to inspect}"

workspace_root="${NEXTEST_WORKSPACE_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
pg_command() {
  if [[ -n "${PG_BIN_DIR:-}" ]]; then
    printf '%s/%s\n' "$PG_BIN_DIR" "$1"
  else
    command -v "$1"
  fi
}
psql="$(pg_command psql)"
createdb="$(pg_command createdb)"
dropdb="$(pg_command dropdb)"
reference="buzz_check_$$_${RANDOM}"
created=false
tmp="$(mktemp -d)"
cleanup() {
  if [[ "$created" == true ]]; then
    "$dropdb" --force --maintenance-db="$BUZZ_POSTGRES_ADMIN_URL" "$reference" || {
      echo "failed to remove owned CHECK reference database: $reference" >&2
      return 1
    }
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

"$createdb" --maintenance-db="$BUZZ_POSTGRES_ADMIN_URL" --template=template0 "$reference"
created=true
if ! "$psql" --no-psqlrc --dbname="$reference" --set=ON_ERROR_STOP=1 \
  --file="$workspace_root/schema/schema.sql" >"$tmp/schema.log" 2>&1; then
  cat "$tmp/schema.log" >&2
  exit 1
fi

# Both databases are on the same server, so pg_get_constraintdef uses the same
# canonical syntax. Compare every public table CHECK, including validation and
# inheritance, rather than a manually maintained list of known omissions.
query="SELECT c.conrelid::regclass::text, c.conname,
              pg_get_constraintdef(c.oid), c.convalidated, c.connoinherit
       FROM pg_constraint c JOIN pg_namespace n ON n.oid = c.connamespace
       WHERE c.contype = 'c' AND n.nspname = 'public'
       ORDER BY 1, 2"
"$psql" --no-psqlrc --dbname="$reference" --set=ON_ERROR_STOP=1 \
  --no-align --tuples-only --command="$query" >"$tmp/expected"
"$psql" --no-psqlrc --dbname="$PGDATABASE" --set=ON_ERROR_STOP=1 \
  --no-align --tuples-only --command="$query" >"$tmp/actual"
if [[ ! -s "$tmp/expected" ]]; then
  echo "direct schema produced no CHECK constraints" >&2
  exit 1
fi
if ! diff -u "$tmp/expected" "$tmp/actual"; then
  echo "test bootstrap CHECK constraints differ from the desired schema" >&2
  exit 1
fi
printf 'PASS: test bootstrap preserves all %s desired CHECK constraints\n' \
  "$(wc -l <"$tmp/expected" | tr -d ' ')"
