#!/usr/bin/env python3
"""Prune GitHub Actions caches for this repository.

Policy (see .github/workflows/cache-prune.yml):
  1. Within each (ref, normalized-key) group, keep the KEEP newest entries
     (most recently accessed) and delete the rest. Normalized keys strip
     hash/date suffixes so successive versions of the same logical cache
     (e.g. v0-rust-...-<sha>-<sha>) group together.
  2. Delete any entry not accessed within MAX_AGE_DAYS, regardless of group.

Requires GH_TOKEN (or GITHUB_TOKEN) with actions:write. Dry-run with --dry-run.
"""

from __future__ import annotations

import argparse
import collections
import json
import os
import re
import subprocess
import sys
import urllib.request
from datetime import datetime, timedelta, timezone

REPO = os.environ.get("GITHUB_REPOSITORY") or subprocess.run(
    ["git", "config", "--get", "remote.origin.url"], capture_output=True, text=True
).stdout

_HASH_RE = re.compile(r"[0-9a-f]{8,40}")


def normalize_key(key: str) -> str:
    key = _HASH_RE.sub("SHA", key)
    key = re.sub(r"\d{6,}", "NUM", key)
    return key


def api(path: str, token: str, method: str = "GET") -> dict:
    req = urllib.request.Request(
        f"https://api.github.com{path}",
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        },
        method=method,
    )
    with urllib.request.urlopen(req) as resp:
        body = resp.read().decode()
        return json.loads(body) if body else {}


def list_caches(token: str, repo: str) -> list[dict]:
    caches: list[dict] = []
    page = 1
    while True:
        data = api(f"/repos/{repo}/actions/caches?per_page=100&page={page}", token)
        batch = data.get("actions_caches", [])
        if not batch:
            break
        caches.extend(batch)
        page += 1
        if page > 50:
            break
    return caches


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default="mfethe1/buzz")
    parser.add_argument("--max-age-days", type=float, default=7.0)
    parser.add_argument("--keep", type=int, default=1, help="entries to keep per (ref, normalized-key) group")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if not token:
        print("::error::GH_TOKEN/GITHUB_TOKEN not set", file=sys.stderr)
        return 1

    caches = list_caches(token, args.repo)
    print(f"listed {len(caches)} cache entries ({sum(c['size_in_bytes'] for c in caches)/1e9:.2f} GB)")

    cutoff = datetime.now(timezone.utc) - timedelta(days=args.max_age_days)

    def last_accessed(c: dict) -> datetime:
        return datetime.fromisoformat(c["last_accessed_at"].replace("Z", "+00:00"))

    groups: dict[tuple[str, str], list[dict]] = collections.defaultdict(list)
    for c in caches:
        groups[(c["ref"], normalize_key(c["key"]))].append(c)

    to_delete: dict[int, str] = {}
    for (ref, nkey), entries in groups.items():
        entries.sort(key=last_accessed, reverse=True)
        for rank, c in enumerate(entries):
            if rank >= args.keep:
                to_delete[c["id"]] = f"dup rank {rank} of {len(entries)} in {ref} [{nkey[:60]}]"
            elif last_accessed(c) < cutoff:
                to_delete[c["id"]] = f"stale ({(datetime.now(timezone.utc) - last_accessed(c)).days}d) in {ref} [{nkey[:60]}]"

    freed = sum(c["size_in_bytes"] for c in caches if c["id"] in to_delete)
    print(f"would delete {len(to_delete)} entries freeing {freed/1e9:.2f} GB" if args.dry_run
          else f"deleting {len(to_delete)} entries freeing {freed/1e9:.2f} GB")

    failures = 0
    for c in caches:
        if c["id"] not in to_delete:
            continue
        reason = to_delete[c["id"]]
        if args.dry_run:
            print(f"[dry-run] {c['key'][:100]} ({c['size_in_bytes']/1e6:.0f} MB): {reason}")
            continue
        try:
            api(f"/repos/{args.repo}/actions/caches/{c['id']}", token, method="DELETE")
            print(f"deleted {c['key'][:100]} ({c['size_in_bytes']/1e6:.0f} MB): {reason}")
        except Exception as exc:  # noqa: BLE001
            failures += 1
            print(f"::warning::failed to delete cache {c['id']} ({c['key'][:60]}): {exc}", file=sys.stderr)

    print(f"done: {len(to_delete) - failures} deleted, {failures} failures, {freed/1e9:.2f} GB freed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
