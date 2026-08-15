# Agent Identity Across Machines

How one owner's four machines stop behaving like four unrelated agents wearing
one name — without putting agent secrets in a relay database forever.

**Status:** design, v2. Nothing here is implemented.
**Related:** [[multi-machine-agent-coordination]]

> **v2 supersedes a rejected v1.** v1 proposed a new kind `30179` carrying the
> agent nsec, NIP-44 self-encrypted, synced through the relay. A 12-agent review
> returned `redesign` from two independent judges at high confidence. Three
> findings, each verified by hand afterwards, killed it:
>
> 1. **The prior art was inverted.** v1 cited NIP-RS (kind 30078) as the model.
>    NIP-RS is one of exactly two coordinates in the tree that *hard*-delete
>    superseded payloads — `let hard_delete_superseded = is_nip_rs ||
>    is_buzz_mesh_status;` (`crates/buzz-db/src/lib.rs:4859`). Every other NIP-33
>    kind soft-deletes (`UPDATE events SET deleted_at = NOW()`,
>    `crates/buzz-db/src/event.rs:820-833`). v1 copied NIP-RS's envelope and
>    dropped the one property that bounded its exposure, while carrying strictly
>    more sensitive content. Revocation would have been cosmetic.
> 2. **The codebase already decided this, the other way.** See §2.
> 3. **The exclusion v1 leaned on does not hold.** See §3.
>
> v1's §2.1 security argument ("the owner nsec is already everywhere, so agent
> nsecs are ~free") is retained in §8 as a *rejected* premise, because the way it
> failed is instructive: it reasoned only about the set of disks and ignored
> permanence, capability asymmetry, and relay multiplicity.

> **Citation policy.** Every `file:line` below was opened and read. Claims I
> could not verify are marked **[unverified]** inline. The companion document
> was written without this discipline and contained fabrications; v1 of this one
> contained two misreads and two imprecise anchors, all corrected below.

---

## 1. The problem, from observed behaviour

An owner signs into Buzz on four machines with one account nsec. They create an
agent on one machine. It appears on all four. They mention it. One machine
answers — or none, if that machine is asleep.

### 1.1 Definitions sync; identities do not

`reconcile_inbound_persona_event` applies the owner's own authored events back
onto local storage, "so Device B inherits Device A's edits"
(`desktop/src-tauri/src/commands/personas/inbound.rs:20-22`).

| Kind | Const | Inbound no-match | Citation |
|------|-------|------------------|----------|
| 30175 | `KIND_PERSONA` | **insert** | `inbound.rs:362` |
| 30176 | `KIND_TEAM` | **insert** | `inbound.rs:413-416` |
| 30177 | `KIND_MANAGED_AGENT` | **no-op** | `inbound.rs:376-380` |

> No match is a no-op: managed agents carry device-local secrets and are never
> minted from a relay event — an agent that does not already exist locally has
> no secret key to run with, so inserting a secretless shell would be useless
> and misleading.
> — `inbound.rs:376-380`

Enforced structurally: the inbound parse returns `ManagedAgentEventContent`, a
type that "physically cannot represent `private_key_nsec`, `auth_tag`,
`env_vars`, `backend`, `agent_command`/`agent_command_override`, or any runtime
field" (`desktop/src-tauri/src/managed_agents/agent_events.rs:124-133`).

So each machine that receives a persona and instantiates it mints a fresh local
keypair. Four machines, one name, four pubkeys.

### 1.2 Turns route by pubkey

The `p`-tag match against the agent's own pubkey is in
`crates/buzz-acp/src/filter.rs:390-395` (`s.first() == Some("p") && s.get(1) ==
Some(agent_pubkey_hex)`), documented at `filter.rs:91-93`. It is on by default
because `--subscribe` defaults to `mentions` (`crates/buzz-acp/src/config.rs:324-330`).

*(v1 cited `config.rs:1262-1280` for this. That range is the
`SubscribeMode::Mentions` arm and contains `require_mention` but no pubkey
comparison. The claim was true; the anchor did not show it.)*

A mention therefore reaches exactly one of the four — whichever pubkey the
sending client resolved. The failure is **routing roulette and silent dead
mentions**.

Duplicate replies are a different path: `--subscribe all` sets
`require_mention: false` for every channel (`config.rs:1282-1291`), and every
local instance fires. Note `subscribe` is a process-level clap arg / env var
(`BUZZ_ACP_SUBSCRIBE`), **not** a per-channel rule — `SubscriptionRule`
(`filter.rs:83-99`) has no such field. *(v1 said "channel rule". Wrong.)*

---

## 2. What this codebase already decided about moving keys

`desktop/src-tauri/src/egress_guard.rs` is a fail-closed guard over relay-bound
egress. Its inventory-completeness test "asserts that every `/events`
URL-construction site in the tree calls this guard, so a new submission path
fails the build until it is wired" (`egress_guard.rs:18-20`).

Its scope note states the architectural position outright:

> Scope: `ncryptsec1` only. The raw `nsec` intentionally transits the
> NIP-44-encrypted pairing session (NIP-AB payload_type "nsec"); guarding it
> here would break pairing. Raw-key DLP is separate policy work.
> — `egress_guard.rs:22-24`

**Key material moves device-to-device over NIP-AB. It does not go to the
relay.** That is not an accident and not an oversight; it is enforced by a build
test and documented at the seam.

v1 would have routed an nsec through `submit_event` and slipped the guard —
NIP-44 ciphertext is base64 and contains no `ncryptsec1` substring, so it would
have passed silently rather than tripping. A design that defeats a fail-closed
guard by accident is the design that is wrong.

NIP-AB is shipped and specified: `crates/buzz-core/src/pairing/`,
`crates/buzz-pair-relay/` (ephemeral sidecar), desktop `start_pairing` /
`confirm_pairing_sas`. `PayloadType` already includes a `Custom` variant —
"Application-defined payload; interpretation is out-of-band"
(`crates/buzz-core/src/pairing/types.rs:63-72`).

---

## 3. The exclusion in `920eced` is not yet a backstop

v1 claimed the already-shipped relay rule "becomes live and meaningful" once
identities unify. That is false on the deployed topology, and it matters because
it is the mechanism the whole design's safety rests on. Three independent
defects:

1. **It defaults off.** `BUZZ_SINGLE_AGENT_CONNECTION` →
   `.unwrap_or(false)` (`crates/buzz-relay/src/config.rs:554-556`). A repo-wide
   grep finds the variable only in `config.rs` — no test, script, or manifest
   sets it.
2. **It is per-process.** `agent_slots` is a `DashMap` on `ConnectionManager`
   (`crates/buzz-relay/src/state.rs:200`), consulted in-process by
   `try_claim_agent_slot` (`:279-308`). Nothing is published to Redis. Under
   horizontal scaling, four machines landing on four pods all claim
   successfully.
3. **It guards one of two doors.** `try_claim_agent_slot` is called from the
   NIP-42 success path (`crates/buzz-relay/src/handlers/auth.rs:268-293`), but
   `buzz-acp` also writes over `POST /events` with NIP-98 auth — including chat
   messages and turn metrics. A harness refused at the socket still replies and
   still bills.

**Consequence, stated plainly: unifying identity onto a flag-off or multi-pod
relay is strictly worse than today.** Today, four keys means one misrouted
reply. Then, four machines on one key means four replies and four bills. Fixing
the exclusion is a hard prerequisite, not a follow-up.

---

## 4. Agent memory changes the argument in both directions

v1 did not mention engrams once. This is the largest omission the review found.

NIP-AE (kind 30174) addresses agent memory by a d-tag derived as an HMAC over
`conversation_key(agent_seckey, owner_pubkey)`
(`crates/buzz-core/src/engram.rs:144-151`), with content encrypted to the same
pair (`:452-472`).

**For unification:** four pubkeys today means four *disjoint memory stores*. The
agent that helped you on the Mac genuinely does not remember it on Windows. This
is the strongest argument for one identity, and v1 missed it entirely.

**Against naive migration:** "promote one key to canonical, retire the others"
destroys three memory stores. Engrams under a retired key are both unaddressable
(d-tag depends on the secret) and undecryptable. Any migration **must
re-encrypt and re-address engrams while the old nsecs are still on disk.** That
is a hard ordering constraint and belongs in the plan, not in a future-work
list.

**New hazard from unification:** one memory head with four concurrent
read-modify-write writers. NIP-AE offers `created_at := max(now, T_head+1)` with
best-effort conflict detection — not a compare-and-set — while `core` is
injected into every new session prompt.

---

## 5. The affinity unit is the session, not the thread

v1 argued handoff is safe because thread context rehydrates from the relay. Two
errors:

- The quoted "never persisted, gone on restart/respawn" (`pool.rs:296`)
  describes `ControlSignal::SwitchModel`'s `desired_model`, **not** session
  state. `SessionState` (`pool.rs:83-92`) carries no persistence statement.
- "Before every prompt" is false. `fetch_conversation_context`
  (`pool.rs:2595-2603`) returns `None` when the event "is a plain channel
  message (not a thread reply, not a DM)". A top-level `@agent do X` gets no
  rehydration at all.

So continuity for plain mentions lives entirely in the in-memory per-channel
session (`sessions: HashMap<Uuid, String>`, `pool.rs:83-92`). Two consecutive
top-level mentions handled by two machines means total amnesia.

**Design constraint:** the unit of affinity is the `(agent, channel)` ACP
session. Mid-task handoff must be *prevented*, not tolerated. This also aligns
with the filesystem boundary — an agent mid-edit in a worktree has nothing on
another machine.

---

## 6. Revised plan

Three stages. Each is independently useful and independently shippable. **Stage
0 is the one to build first, and it needs no decision about secrets at all.**

### Stage 0 — machine identity and visibility

Make the four instances visibly distinct and attributable. No new kind, no
secrets moved, no relay changes.

v1 marked "no existing machine-identity concept" as `[unverified]`. It was
wrong: `desktop/src-tauri/src/mesh_llm/mod.rs:86-114` defines `MeshServeTarget`
with `device_id`, `device_name`, `node_name`, and `capacity { vram_gb }`, and
documents `owner_id` as the "per-runtime MeshLLM owner identity… Distinguishes
two devices logged into the same Buzz member account." Generalize that rather
than inventing one.

Deliverables: per-instance machine label in the agent list and in mentions;
per-instance presence; attribution of which machine answered.

This converts §1's silent dead mention into a visible choice, which is most of
the felt problem. It is also a strict prerequisite for debugging *either*
identity design.

### Stage 1 — make the exclusion real

Fix all three defects in §3: a Redis-backed lease (the `KIND_PUSH_LEASE` /
kind 30350 author-only lease is the in-repo precedent), coverage of the
`POST /events` path, and a decision on the default-off flag — either flip it or
advertise the capability via NIP-11 so clients can gate on it.

Add liveness: a refused harness must be able to take over when the holder
disappears. The acceptance criterion is **the measured worst-case dead-mention
window after the holder sleeps**; today the honest answer is unbounded, because
`sync_managed_agent_processes`
(`desktop/src-tauri/src/managed_agents/runtime/lifecycle.rs`) only reaps exited
harnesses and nothing respawns one whose startup was refused.

### Stage 2 — identity transfer over NIP-AB

Transfer the agent keypair **device-to-device over the tailnet**, using the
pairing channel the codebase already sanctions for exactly this (§2), with
`PayloadType::Custom`.

This is a better fit than v1's relay event on every axis that killed v1:

| | v1 (relay kind 30179) | v2 (NIP-AB direct) |
|---|---|---|
| Ciphertext at rest | forever, soft-delete only | none |
| Revocation | cosmetic | n/a — nothing stored |
| Egress guard | silently defeated | sanctioned path |
| Multi-relay | secret to every relay used | not applicable |
| Scope match | permanent head for a one-shot bootstrap | one-time transfer, exactly |

Do **not** carry `auth_tag`. Recompute it on arrival, as import already does
(`desktop/src-tauri/src/commands/personas/snapshot/import.rs:514-532`), so the
payload is not a self-contained bearer capability.

**Apply rule — repair-once, not write-once.** Apply when the local record has no
secret **or** when the stored nsec derives a pubkey different from the record's
(the check `resolve_unlock_secret` already performs,
`agent_snapshot_envelope.rs:284-288`). Write-once keyed on "non-empty" would
permanently wedge a machine holding a wrong key.

**Third state:** on desktop, `private_key_nsec` is legitimately empty until
keyring hydration and stays empty in the `keyring_locked` boot state. Bootstrap
must **refuse**, not proceed, when the secret store is unreachable.

**Known gap:** a headless machine cannot do an SAS confirmation. If a
relay-carried path is ever needed, it should be scoped to that case and carry
the §2 consequences explicitly — not be the default for all four machines.

---

## 7. Test plan

Retargeted per the review; v1's templates were wrong in three places.

Desktop unit (`cargo test --manifest-path desktop/src-tauri/Cargo.toml` — the
desktop crate is excluded from the root workspace):

1. NIP-AB `Custom` payload round-trip. The reusable Rust pattern is
   `managed_agents/agent_snapshot_envelope.rs:221` (encrypt) / `:312` (decrypt),
   **not** NIP-RS — kind 30078 self-encryption is implemented in TypeScript
   (`desktop/src/features/channels/readState/`), so there is no Rust prior art
   to copy there.
2. Inbound structural secret-drop. Template is
   `from_event_drops_injected_secret_and_harness_keys`
   (`agent_events.rs:391-426`), **not** the outbound projection test at `:248` —
   that one narrows a wide `ManagedAgentRecord`, a mechanism a hand-written
   payload does not have, so mirroring it would pass vacuously.
3. Pubkey/d-tag mismatch rejected.
4. Repair-once: applies when absent, applies when wrong, no-ops when correct.
5. `keyring_locked` refuses bootstrap.
6. Engram re-address/re-encrypt preserves memory across a key change (§4).

Relay / integration (`just test`, requires Postgres + Redis):

7. Lease is honoured across two relay processes sharing Redis (§3 defect 2).
8. A harness refused at the socket cannot write via `POST /events` (§3 defect 3).
9. Failover: holder disconnects, standby serving within N seconds — N is the
   acceptance criterion from §6 Stage 1.

Gate: `just ci`, plus `just test` because this touches `buzz-relay`.

**Not needed in v2:** the `AUTHOR_ONLY_KINDS` addition and its migration. v1
treated that as a one-line const change; it is not — `crates/buzz-search/tests/
fts_integration.rs` asserts the const matches the storage-level `search_tsv`
skip-set, and the precedent (`migrations/0014_push_lease_fts.sql`) requires
dropping and re-adding a `GENERATED ALWAYS … STORED` column and rebuilding the
GIN index: a full `events` rewrite under `ACCESS EXCLUSIVE`, auto-applied on
relay startup. Dropping the relay event drops this entire problem.

---

## 8. Premises

**Rejected (v1):** *"The owner nsec is already on every machine, so syncing
agent nsecs adds ~nothing."* It reasoned only about the set of disks. It missed:
permanence (§2 banner), **capability asymmetry** — `try_claim_agent_slot` keys
on the *agent* pubkey, a capability the owner nsec does not confer, so one
compromised machine can win the exclusive slot and lock out the honest fleet —
and relay multiplicity, since managed agents are stored in one global
`managed-agents.json` but a relay event publishes to whichever community relay
is active.

**Still standing, still attackable:**

1. One shared identity beats distinct identities behind a routing alias. §4's
   memory argument is the strongest support; Stage 0 delivers most of the mention
   UX benefit without it.
2. `(agent, channel)` is the right affinity unit (§5).
3. Stage 0 is worth shipping before either identity decision.

---

## 9. Work not yet scoped

Surfaces that assume one pubkey means one process, each of which breaks under
unification. **[unverified — reported by review, not personally traced]**:

- **Presence:** a single Redis key per pubkey with an unrefcounted `DEL` on the
  kind:20001 offline path — one machine quitting marks the whole fleet offline.
- **Observer frames:** a process-local `seq` starting at 1, merged desktop-side
  per agent pubkey ordered by `(timestamp, seq)` — an unlabelled interleave of
  four machines.
- **Observer control frames** (`cancel_turn`, `switch_model`): addressed by agent
  pubkey alone, so they fan out to all four.
- **kind:10100 agent profile:** replaceable with no d-tag, so it cannot be
  partitioned per machine.
- **Rate limits** keyed on pubkey collapse N budgets into one.

Also unscoped: mobile's position (should a phone ever hold agent identity —
probably not, needs a stated answer); billing semantics; migration for fleets
already fragmented; orphaned harnesses (one `buzz-acp` was observed outliving
its desktop parent — under a lease regime an orphan holding a lease is worse
than one wasting a socket).

`managed_agents/runtime_commands.rs:119-132`'s `start_nonce` generation check is
the existing precedent for a machine discriminator.

---

## 10. Open questions

1. Does boot reconcile refetch all three synced kinds unconditionally? **[unverified]**
2. Rotation: NIP-IA kind:9035 archive requests already carry `reason: rotated`
   and a `replaced-by` field (`docs/nips/NIP-IA.md:104-105`). Does that subsume
   rotation here? Note a `reason=rotated` archive is not a NIP-09 tombstone.
3. Flip `BUZZ_SINGLE_AGENT_CONNECTION`, or advertise via NIP-11 and gate
   clients?
4. Does the fleet ever include a machine that should not run agents?
5. Is Stage 0 alone enough? It is plausible that visible, attributable
   per-machine instances solve the felt problem and Stages 1–2 are never needed.

---

Sources: verified against the tree at `design/tailnet-agent-mesh`; v2 informed
by workflow run `wf_03db70bf-f25` (12 agents, two independent `redesign` verdicts)
Last updated: 2026-08-15
Related: [[multi-machine-agent-coordination]]
