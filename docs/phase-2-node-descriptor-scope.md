# Phase 2 Scope — Node Descriptor and "Your machines"

Status: scope, not implementation. Companion to
[`multi-machine-agent-coordination.md`](multi-machine-agent-coordination.md)
§ "Phase 2 — Know your machines". **Read
[`agent-identity-sync.md`](agent-identity-sync.md) first** — it is the v2 design
for the identity half of this problem, it holds the reason kind 30179 is skipped
(§2 below), and it independently corroborates the `hard_delete_superseded`
anchor at `crates/buzz-db/src/lib.rs:4859` found here. Its purpose is to make that phase's touch list
actionable: every anchor below was opened and checked on this branch, and the
ones that were wrong are corrected here rather than in place.

**Goal, unchanged:** a Settings → "Your machines" panel and a per-machine
Schedulable toggle. No placement logic. The toggle alone converts "four machines
clashing" into "the Air only helps when I say so".

---

## 1. Citation audit

The design doc carries a standing warning that its `file:line` citations are
unaudited. Every Phase 2 anchor was checked. Results:

| Doc claim | Reality | Impact |
|---|---|---|
| `KIND_NODE_DESCRIPTOR = 30180` | **Free.** Nothing in `kind.rs` uses 30180 | none — proceed |
| `AUTHOR_ONLY_KINDS` is `[KIND_EVENT_REMINDER, KIND_PUSH_LEASE]` @ `kind.rs:120` | **Exact** | none |
| `getrandom` already a dependency | **True** — `desktop/src-tauri/Cargo.toml:93`, `getrandom = "0.2"` | none |
| `effective_repos_dir()` @ `repos.rs:186` | **Exact** | none |
| `Requirement` @ `readiness.rs:295` | `readiness.rs:294` | trivial |
| `discover_acp_runtimes_from` @ `discovery.rs:1474` | `discovery.rs:1459` | trivial |
| `required_scope_for_kind` @ `ingest.rs:345` | `ingest.rs:211` | trivial |
| `is_global_only_kind` @ `ingest.rs:529` | `ingest.rs:395` | trivial |
| `hard_delete_superseded` @ `buzz-db/src/lib.rs:5210`, "add 30180 to" it | `lib.rs:4859`. **It is not a list.** It is `let hard_delete_superseded = is_nip_rs \|\| is_buzz_mesh_status;`, each a predicate over kind + `d_tag` shape | **mechanism differs** — see §3.4 |
| `schema/schema.sql:224` is the search denylist | Denylist is real but at **`schema.sql:215`**; `:224` is mid-DDL for the `events` table | **see §3.5 — the substance of the warning is correct and important** |
| New module "modeled on `private_managed_agent.rs`" | **No such file exists** anywhere in the tree | **model replaced** — see §3.2 |
| 30180 goes in `AUTHOR_ONLY_KINDS` "next to `KIND_PRIVATE_MANAGED_AGENT`" | **`KIND_PRIVATE_MANAGED_AGENT` does not exist** in `kind.rs` | cosmetic, but signals the same fabricated cluster |
| `sysinfo` is "the only new dependency" | Already in `Cargo.lock` **twice** (0.37.2 and 0.38.4) as transitive deps | **not new third-party code** — see §3.3 |

Two of these are the same fabrication family the doc's own audit note describes
(a `private_managed_agent` / `NIP-PMA` cluster that does not exist). Treat any
remaining un-checked citation in that document with the same suspicion.

---

## 2. Kind number: 30180 is correct — do not "reclaim" 30179

**Corrected 2026-08-16.** An earlier revision of this scope recommended taking
`30179` to keep the addressable agent block dense (`30174` engram, `30175`
persona, `30176` team, `30177` managed agent, `30178` team catalog). That
recommendation was wrong and is withdrawn.

`docs/agent-identity-sync.md` — which this scope failed to read — records that
**`30179` was the kind proposed by that document's rejected v1**, carrying the
agent nsec NIP-44 self-encrypted through the relay. A 12-agent review returned
`redesign` from two independent judges, on three verified findings (the lead one:
v1 copied NIP-RS's envelope while dropping the hard-delete property that bounded
its exposure, so revocation would have been cosmetic).

So the "hole" at 30179 is deliberate: a burned number attached to a rejected
key-transport design. Reusing it would collide with any implementation, notes, or
review threads that still refer to 30179 as "the one that carries the nsec" —
exactly the confusion a fresh number avoids.

**`KIND_NODE_DESCRIPTOR = 30180`, as originally designed. No decision required.**
The NIP should state why 30179 is skipped so this is not re-opened a third time.

---

## 3. Corrected work breakdown

### 3.1 `crates/buzz-core/src/kind.rs`

Add the constant, add it to `ALL_KINDS` (`:622`) and `AUTHOR_ONLY_KINDS`
(`:120`). Note `ALL_KINDS` has a round-trip test at `:890` that iterates every
kind — a new entry must satisfy whatever that asserts.

**Size:** minutes. **Risk:** none, except the tripwire in §3.5 which fires on the
`AUTHOR_ONLY_KINDS` change.

### 3.2 `crates/buzz-core/src/node_descriptor.rs` (new)

Types, NIP-44 self-encrypt/decrypt, d-tag build/parse validating `node:<32 hex>`.

The doc's stated model does not exist. **Use `engram.rs` instead** — it is the
true analogue: addressable, NIP-44 self-encrypted, adjacent kind (30174), same
owner→owner shape. `observer.rs` (159 lines) is the lighter reference for the
encrypt/decrypt helpers alone; `engram.rs` is 1049 lines and carries far more
than this needs, so copy its *shape*, not its bulk.

**Size:** the largest single unit. Budget on the order of `observer.rs` plus
d-tag validation and the payload structs from the design doc, not on `engram.rs`.

### 3.3 Dependency: `sysinfo` for `ram_total_mb`

The doc frames this as the one new dependency. It is **already in the lock file
twice** (0.37.2, 0.38.4) via transitive paths, so promoting it to a direct
dependency of `desktop/src-tauri` adds no new third-party code — it is a version
*pinning* decision, and picking 0.38.4 likely lets one of the two duplicates be
unified.

Worth asking whether one `u64` justifies a direct dependency at all. Every other
fact in the payload comes from `std` (`std::env::consts`,
`std::thread::available_parallelism`). If `ram_total_mb` can be `Option<u64>`
and omitted on platforms without a cheap read, the dependency disappears
entirely and the rank function in Phase 3 treats it as unknown. **Flagged as a
decision, not a recommendation** — Phase 3's rank may genuinely need it.

### 3.4 `crates/buzz-db/src/lib.rs` — history suppression

The doc says "add 30180 to `hard_delete_superseded`". There is no list to add
to. At `lib.rs:4859`:

```rust
let hard_delete_superseded = is_nip_rs || is_buzz_mesh_status;
```

`is_nip_rs` and `is_buzz_mesh_status` are each predicates over the kind plus the
`d_tag` shape (and for the mesh case, a `k` tag). So the change is a **third
predicate term**, `is_node_descriptor`, matching kind + `d_tag` prefix `node:`.
That is more code than an array append and it sits in the hot
`replace_parameterized_event` path — it needs its own test alongside the
existing two.

The doc's claim that no migration is needed still holds: a brand-new kind has no
accumulated history to backfill.

### 3.5 Search allowlist / denylist reconciliation — the real hazard

The doc's line number is wrong but **its warning is correct and is the most
easily-missed item in the phase.** The two paths genuinely disagree:

- `schema/schema.sql:215` (fresh installs) is a **denylist**:
  `CASE WHEN kind IN (1059, 30300, 30350, 30622, 44100, 44101, 44200) THEN NULL::tsvector ELSE to_tsvector(...)`
  → anything *not* listed **is searchable**.
- `migrations/0008_fresh_install_search_allowlist.sql` is an **allowlist**:
  `CASE WHEN kind IN (0, 9, 40002, 45001, 45003) THEN to_tsvector(...) ELSE NULL`
  → anything *not* listed **is not searchable**.

For a new author-only kind these produce **opposite** results: a fresh install
would index node descriptors into full-text search, while a migrated database
would not. Since the content is NIP-44 ciphertext the leak is limited, but an
author-only kind being FTS-indexed on one install path and not the other is
exactly the inconsistency the tripwire test
`author_only_kinds_are_storage_level_unsearchable`
(`crates/buzz-search/tests/fts_integration.rs:1268`) exists to catch.

**So this is not optional bookkeeping — `schema.sql` must gain the new kind in
its denylist or the tripwire fails.** Budget it as real work with a test, and
consider whether the two paths should be reconciled to one policy while someone
is in there. That reconciliation is arguably its own PR.

### 3.6 `crates/buzz-relay/src/handlers/ingest.rs`

`Scope::UsersWrite` in `required_scope_for_kind` (`:211`); add to
`is_global_only_kind` (`:395`). Both functions exist as described.

**Size:** small. **Risk:** low, but `is_global_only_kind` is the same mechanism
that makes NIP-34 kinds discard a stray `h` tag — confirm that is the intent for
a per-machine record before copying it.

### 3.7 Desktop Rust: `device/identity.rs`, `device/facts.rs`, `device/announce.rs` (new)

Anchors confirmed: `discover_acp_runtimes_from` (`discovery.rs:1459`),
`effective_repos_dir()` (`repos.rs:186`), `Requirement` (`readiness.rs:294`).
`getrandom` is already available for the device id.

The design doc's own constraints that must survive into implementation:
- facts cached at launch plus explicit refresh, **never on a timer** — process
  spawn is expensive on Windows;
- the publisher is a tokio task and must be **explicitly torn down and rebound**
  on community switch, because React key-remounting does not touch it;
- `HostPolicy` is one per-machine record published identically to every relay,
  not per-community.

### 3.8 Desktop frontend: `fleetStore.ts`, `YourMachinesSection.tsx` (new)

`resetCommunityState` exists at `useCommunityInit.ts:48` and is invoked at
`:195`; `resetFleetStore()` must be registered there — this repo's CLAUDE.md
calls out module-level singletons leaking across community switches as a known
failure mode.

Liveness must derive from **local receive time**, not `created_at` — the design
doc's reasoning (remote wall clocks, no timestamp bound on ingest) is sound and
should be preserved verbatim in the NIP.

Text sizing: rem-based tokens only; `pnpm check:px-text` will fail the build
otherwise.

### 3.9 `docs/nips/NIP-ND.md` (new)

Kind number, d-tag layout, payload schema, cadence, author-only read rule. Should
also record the 30179-vs-30180 decision from §2 and the local-receive-time
liveness rule, so neither is re-litigated.

---

## 4. Suggested landing order

Each step is independently reviewable; 1–3 are backend-only and ship without UI.

1. **Kind + core type** (§3.1, §3.2) — constant, `node_descriptor.rs`, unit tests.
2. **Storage + search** (§3.4, §3.5) — the `hard_delete_superseded` predicate and
   the allowlist/denylist reconciliation, with the tripwire green.
3. **Relay ingest** (§3.6).
4. **Desktop publisher** (§3.7) — device id, facts, announce.
5. **Panel** (§3.8) + **NIP** (§3.9).

Splitting after step 2 is worth considering: the search reconciliation touches a
shared invariant and may deserve to land alone.

---

## 5. Decisions needed before implementation

1. ~~Kind number~~ — **settled**, 30180. See §2; 30179 is burned by the rejected
   v1 of `docs/agent-identity-sync.md`.
2. **`sysinfo`** — direct dependency for one field, or `Option<u64>` and drop it. §3.3.
3. **Search paths** — patch `schema.sql`'s denylist only, or reconcile the two
   install paths to one policy in a separate PR. §3.5.

## 6. Done when

Unchanged from the design doc: all four machines appear in the panel with
correct harness chips including which credential each would bill; toggling
Schedulable off on one machine persists and is visible from another within 60s;
a community switch leaves no stale rows and no stale publisher. Plus `just ci`
green and the FTS tripwire passing.

---
Sources: every anchor in §1 opened on branch `worktree-phase2-node-descriptor-scope` at `f2d7c01`
Last updated: 2026-08-15
Related: [[multi-machine-agent-coordination]]
