# Multi-Machine Agent Coordination over a Tailnet

Status: design, not implemented. Supersedes nothing; adds one event kind and one
relay rule. Written for one person's four personal machines, and deliberately
sized for that.

> **Citation health.** The first draft of this document was machine-synthesised
> and its `file:line` citations were not individually checked before commit. A
> later audit found fabrications — a non-existent `NIP-PMA` / kind `30179`, and
> a claim that the agent snapshot envelope carries the agent nsec (it does the
> opposite; see "State B" below). Those are corrected. **The remaining citations
> are still unaudited.** Verify any line you intend to act on: kind integers
> against `crates/buzz-core/src/kind.rs`, NIP names against `ls docs/nips/`, and
> every `path:line` by opening it. Treat the argument as the contribution and
> the citations as leads.

---

## The problem

One Buzz account. Four machines on one Tailscale tailnet: a Windows desktop, two
Mac minis, a MacBook Air. The same agents are configured on all four, because
agent *definitions* replicate: `apply_inbound_persona`
(`desktop/src-tauri/src/commands/personas/inbound.rs:369`) INSERTs a kind:30175
persona on no-match, so every machine that signs in inherits every definition.

What does *not* replicate is the runnable instance.
`apply_inbound_managed_agent` (`inbound.rs:408`) is a deliberate no-op on
no-match, because the instance carries an nsec that lives in the local keyring
and never crosses the wire (`managed_agents/agent_events.rs` module doc lists
nsec/auth_tag/env_vars/backend as never-published). So the user ends up in one of
two states, and both are broken:

**State A — four sibling identities.** The user creates the agent separately on
each machine. `create_managed_agent` calls `Keys::generate()`
(`desktop/src-tauri/src/commands/agents.rs:624`), so "Ada" is four distinct
Nostr pubkeys, four distinct `users` rows, four avatars in the member list.
`require_mention` matches against the harness's own `agent_pubkey_hex`
(`crates/buzz-acp/src/filter.rs:390`), so an @-mention reaches exactly one of
them — the user has to remember which Ada is which machine. And for any rule
with `require_mention: false` (`filter.rs:122` — the default), all four match
the same channel message and all four answer. Four replies, four bills.

**State B — one identity, four copies.** One pubkey live on four machines. All
four harnesses authenticate to the relay, all four subscribe, all four match,
all four reply. buzz-acp's dedup is a process-local `seen_membership_ids` set
(`crates/buzz-acp/src/lib.rs:2173`) and cannot see the other three processes.

**State B is not reachable today, and that is the more important finding.** No
shipped surface moves an agent identity between machines. The agent snapshot
excludes secrets *by construction* — `private_key_nsec`, `auth_tag`, `env_vars`
and `relay_url` are on an explicit never-serialized list
(`managed_agents/agent_snapshot.rs:19-34`), enforced by
`import_source_identity_fields_never_consumed`
(`commands/personas/snapshot/tests.rs:795-830`). Import mints a fresh keypair
and a fresh NIP-OA auth tag bound to it (`commands/personas/snapshot/import.rs:514-532`),
so importing the same file twice yields two distinct agents. The NIP-AE
envelope is encryption-at-rest for a *shareable card*, not an identity
transport: `resolve_unlock_secret` (`agent_snapshot_envelope.rs:271-289`) reads
the importing machine's own record to derive a NIP-44 decryption key, so the
nsec is an unlock input, never envelope payload. And `buzz-cli` has no
export/import equivalent. Definitions replicate over the relay
(`personas/inbound.rs:369`); identities deliberately do not
(`personas/inbound.rs:408` is a no-op).

So a four-machine fleet is necessarily in **State A**, and the two states need
different fixes. State B is a *collision* — two claimants to one identity — and
is settled by exclusion at the relay. State A is *fragmentation* — four
identities with one name — and exclusion cannot touch it, because there is no
shared key to exclude on. Relay-side one-socket-per-pubkey is therefore a
precondition for identity mobility rather than a fix for the fleet as it
exists: it makes State B safe to allow, but nothing today allows it.

Nothing arbitrates either state. An exhaustive search for a lock, lease, claim,
or election in the agent path finds three `std::sync::Mutex` fields in
`desktop/src-tauri/src/app_state.rs:51-54` (in-process only), an in-process
worker-pool `try_claim` in `crates/buzz-acp/src/pool.rs`, and nothing else. The
relay imposes no one-connection-per-agent-pubkey rule.

Compounding it: on every boot, `restore_managed_agents_on_launch`
(`managed_agents/restore.rs:168-172`) starts every record with
`start_on_app_launch && backend == Local`, and
`reconcile_managed_agent_runtimes` (`managed_agents/runtime_commands.rs:459`)
fans each one out to *every* configured community. Four machines, four boots,
no coordination.

And nothing knows what any machine can do. The one hardware probe in the tree is
`mesh_llm_system::hardware::survey()` (`desktop/src-tauri/src/mesh_llm/catalog.rs:119`)
returning `gpu_name` + `vram_bytes`, behind a cargo feature that is not in
`default` (`desktop/src-tauri/Cargo.toml:25-26`). There is no RAM, core count,
disk, or OS/arch anywhere. `discover_acp_runtimes_from`
(`managed_agents/discovery.rs:1474`) computes exactly the right per-machine
facts — which harnesses are installed, adapter-adequate, logged in, and *which
credential they bill* (`AuthCredential`, `managed_agents/types.rs:631`, the
subject of commits 47f8ceb and 79a55b9) — and then throws them away at the
window boundary. No peer can see them. And every agent on every machine runs
with `cwd = ~/.buzz` (`managed_agents/mod.rs:101`, applied at
`managed_agents/runtime.rs:583`), so "which machine has the right checkout" is
not merely unpublished, it is unrepresentable.

Concretely, today: the user types `@ada fix the flaky test in buzz-relay` and
either gets one reply from a machine that has no clone of the repo, or gets
three identical replies with three separate Claude subscriptions billed.

---

## What already exists

**A relay every machine already holds open.** All four machines maintain an
authenticated NIP-42 WebSocket. This is the only link guaranteed to work, it is
the only thing that can serialize, and it already has liveness detection:
`heartbeat_loop` (`crates/buzz-relay/src/connection.rs:424-455`) pings every 30s
and closes after 3 missed pongs. Dead-connection detection is therefore bounded
at ~90s today, free, with no protocol.

**Per-connection authenticated identity, already tracked.** `ConnectionManager`
holds `authenticated_pubkey: Arc<RwLock<Option<Vec<u8>>>>`
(`crates/buzz-relay/src/state.rs:109`), set at
`set_authenticated_pubkey` (`state.rs:308`), with
`disconnect_pubkey` (`state.rs:371`) and a cluster-wide variant
(`state.rs:1161`) already shipping. *Limit:* it tracks connections; it enforces
no cardinality rule. Multiple connections per pubkey are explicitly supported
and tested (`state.rs:1550`).

**Owner-scoped cross-machine telemetry.** `KIND_AGENT_OBSERVER_FRAME` (24200) —
NIP-44 frames `#p`-tagged to the owner, consumed by every signed-in machine
(`desktop/src/shared/api/observerRelay.ts`,
`desktop/src/features/agents/observerRelayStore.ts:487`), folded into
`activeAgentTurnsStore.ts` with per-agent clock-offset estimation and terminal
tombstones. *Limit:* routing is signer-classified —
`agent_observer_route` (`crates/buzz-relay/src/handlers/event.rs:1103-1141`)
accepts agent→owner as telemetry and owner→agent as control, and *silently
drops* (`Ok(None)`, `:1131-1134`) any frame tag that is neither. The client
mirrors this: `observerRelayStore.ts:511-513` returns early unless
`event.pubkey === agentPubkey`. A desktop cannot publish an owner-signed frame
into this stream. Frames also carry no host identity.

**A per-machine harness capability report, computed every launch.**
`discover_acp_runtimes_from` (`managed_agents/discovery.rs:1474`) →
`AcpAvailabilityStatus` / `AuthStatus` / `AuthCredential`
(`managed_agents/types.rs:587,601,631`), plus `agent_readiness`
(`managed_agents/readiness.rs:403`) returning
`AgentReadiness::NotReady { requirements }` with typed variants. *Limit:*
returned only to the local window; no event kind, no schema, no publish path.

**A per-installation coordinate the relay already accepts.** NIP-RS read-state
uses `d = read-state:<32 lowercase hex>` generated once per install
(`desktop/src/features/channels/readState/readStateManager.ts:332`), structurally
validated at `crates/buzz-db/src/lib.rs:5200-5209`. This is the precedent for
giving one account several non-colliding NIP-33 coordinates. *Limit:* it is a
read-position blob, not a device record.

**NIP-34 repo coordinates.** `KIND_GIT_REPO_ANNOUNCEMENT` (30617) /
`KIND_GIT_REPO_STATE` (30618) at `crates/buzz-core/src/kind.rs:604-607`;
`30617:<owner-hex>:<repo-d>` is already the git ACL anchor via the
`buzz-channel` tag (`crates/buzz-relay/src/api/git/binding.rs`). A canonical,
path-free repo identifier already exists. *Limit:* all NIP-34 kinds are in
`is_global_only_kind` (`crates/buzz-relay/src/handlers/ingest.rs:568-577`, and
`ingest.rs:2237-2239` unconditionally sets `channel_id = None`) — a stray `h`
tag is discarded, so a PR event cannot land in a channel timeline.

**Agents already inherit the user's git credentials.** `spawn_agent_child`
(`managed_agents/runtime.rs:466`) does *not* call `env_clear` (verified: zero
occurrences in the file), so the child inherits ssh-agent, `GH_TOKEN`, and the
rest. Separately, `runtime.rs:841-871` injects a `git-credential-nostr` helper
for the relay git remote. Cloning and pushing to a real GitHub origin works
today with no new machinery.

**A repos root per machine.** `validate_repos_dir` / `ensure_repos_symlink` /
`effective_repos_dir` (`managed_agents/repos.rs:23,76,186`). *Limit:* the
`#[cfg(not(unix))]` arm (`repos.rs:144-148`) ignores the configured `repos_dir`
entirely and just `create_dir_all`s an empty `REPOS` — the Windows machine
literally cannot point REPOS at an existing checkout root today.

**Fenced-lease prior art, unreachable from a client.** `SessionDirectory`
(`crates/buzz-relay/src/tunnel/directory.rs:17-83`) — Redis Lua acquire/renew/
release/validate with a monotonic generation, and the doctrine we adopt
verbatim: *membership is a hint; the fence is the arbiter*
(`crates/buzz-relay-mesh/src/lib.rs:18-19`). *Limit:* relay-mesh-internal, keyed
on `RuntimeId`/`Profile`, no Nostr surface, no client path.

**Things that look reusable and are not.** `current_instance_id()`
(`managed_agents/runtime/process.rs:134`) is `app.config().identifier` — the
Tauri bundle id, byte-identical on all four machines. `mesh_llm/identity.rs:111-149`
has a genuine per-device ed25519 identity but lives behind the non-default
`mesh-llm` feature at a path the mesh-llm SDK owns. `buzz-relay-mesh` is not a
dependency of `desktop/src-tauri/Cargo.toml` at all, its registry is raw Redis,
and its membership is fail-closed against the server-only
`BUZZ_RELAY_PRIVATE_KEY` — a desktop cannot join it.

---

## Design overview

The relay is the arbiter, because it is the one component all four machines
already trust and already connect to, and because a peer-to-peer link cannot
arbitrate mutual exclusion. Exclusion is enforced at the *connection*, not at the
turn: the relay permits exactly one authenticated agent WebSocket per
`(community, agent pubkey)`, so the second and third and fourth harness are
rejected at AUTH with a NOTICE naming the incumbent — no lease, no generation, no
new table, and the existing 30s ping / 3-missed-pong loop is the liveness
detector. Placement is therefore not an assignment but an *ordering*: before a
machine opens its agent connection it waits a delay derived from a purely
integer tier ladder (pin → which credential gets billed → is the workspace here →
is the harness warm → how many agents am I already running → hash tiebreak), so
the best-placed machine reaches AUTH first and becomes the incumbent, and the
others back off. Divergence between machines' views is harmless by construction:
if two pick the same delay they simply race, and the relay resolves it — nothing
requires the four machines to agree on anything. Each machine publishes one
addressable, owner-encrypted node descriptor (kind 30180, `d = node:<device-id>`)
whose device id sits in the d-tag so four machines on one account get four
non-colliding NIP-33 coordinates rather than clobbering one; liveness of a peer
descriptor is judged by *local receive time*, never by `created_at`. Workspaces
are named by the NIP-34 coordinate `30617:<owner>:<repo-d>` — path-free, so
Windows and macOS agree — and resolved to a per-agent durable worktree locally,
because the ACP harness captures its cwd once at process start
(`crates/buzz-acp/src/lib.rs:2030`) and cannot be given a different directory per
turn. Tailscale carries nothing: its only job is letting `BUZZ_RELAY_URL` be a
MagicDNS name if the relay is self-hosted. The whole thing is one new event kind,
one relay cardinality rule, and one connect-delay function.

```
                     ┌──────────────────────────────────────┐
                     │              RELAY                   │
                     │                                      │
                     │  ConnectionManager                   │
                     │    (community, agent_pk) -> 1 conn   │  <-- the arbiter
                     │    state.rs:109 / :308 / :371        │
                     │                                      │
                     │  events: 30180 node descriptors      │
                     │          24200 observer frames       │
                     │          30617 repo announcements    │
                     └───▲────────▲─────────▲────────▲──────┘
      AUTH (accepted)    │        │         │        │   AUTH (rejected:
      + agent traffic    │        │         │        │    slot held by <device>)
                         │        │         │        │
                ┌────────┴──┐ ┌───┴─────┐ ┌─┴───────┐ ┌┴──────────┐
                │ mini-1    │ │ mini-2  │ │ windows │ │ macbook   │
                │ (AC)      │ │ (AC)    │ │ (AC)    │ │ air       │
                │ rank 0    │ │ rank 1  │ │ rank 2  │ │ rank 3    │
                │ delay 0s  │ │ delay 2s│ │ delay 4s│ │ delay 6s  │
                │           │ │         │ │         │ │           │
                │ buzz-acp  │ │ (idle)  │ │ (idle)  │ │ (idle)    │
                │  RUNNING  │ │ backoff │ │ backoff │ │ backoff   │
                └───────────┘ └─────────┘ └─────────┘ └───────────┘
                      │
                      └─► git push  ──►  github.com origin (inherited creds)

   Every machine publishes its own 30180 every 60s / on change.
   Every machine reads all four 30180s -> computes only ITS OWN rank -> delay.
   No machine needs to agree with any other. The relay decides.
   Tailscale (if present) carries only BUZZ_RELAY_URL to a self-hosted relay.
```

---

## Node identity and capability advertisement

### Identity: a random per-installation id, not a keypair

`<app_data_dir>/device.json`, mode 0600, containing 32 lowercase hex from
`getrandom` (already a dependency), a user-editable display name defaulting to
the hostname, and a creation timestamp. Not feature-gated.

**Not a keypair, deliberately.** All four machines hold the same account nsec, so
a per-device signature proves nothing an attacker with that nsec could not forge.
A device keypair here is ceremony. If the fleet ever spans more than one person's
trust boundary, `mesh_llm/identity.rs:55-85`'s owner-binding signature scheme is
the ready-made upgrade — the mechanism ports, only the anchor changes.

**Rejected: the mesh-llm `owner_id`** (`desktop/src-tauri/src/mesh_llm/identity.rs:111-149`).
It is the right shape but lives behind the `mesh-llm` cargo feature, which is
absent from `default = ["system-keyring"]` (`desktop/src-tauri/Cargo.toml:25-26`).
`just dev`, `just staging`, and `just production` (`justfile:9`) all build without
it, so on a dev build every mesh command returns "mesh-llm feature not enabled"
(`mesh_llm_stubs.rs`). A coordination layer that silently no-ops on dev builds is
not a coordination layer. The keystore path is also owned by the mesh-llm SDK.
No cross-reference field is carried; it would have no reader and would make one
descriptor field vary between dev and release builds.

**Rejected: `current_instance_id()`** (`managed_agents/runtime/process.rs:134`) —
`app.config().identifier`, identical on all four machines. It distinguishes
builds, never hosts.

**Clone guard.** A Time Machine restore or VM clone can carry `device.json` to a
second box, and two machines sharing a `device_id` share one 30180 coordinate and
silently overwrite each other. The detector is free: we already subscribe to
descriptors at our own coordinate. If a descriptor arrives at
`30180:<self>:node:<my_id>` whose content hash is not one we published, another
box holds our id — rotate to a fresh id and publish a kind:5 for the old
coordinate. Roughly ten lines, and it catches convergence by *any* route.
**Rejected:** hashing the OS machine id (IOPlatformUUID / MachineGuid /
`/etc/machine-id`) — three platform implementations plus an undeclared `winreg`
dependency to detect a subset of what the self-coordinate check catches for free.

### The descriptor: one addressable kind, no ephemeral heartbeat

`KIND_NODE_DESCRIPTOR = 30180`, addressable at `d = node:<32 lowercase hex>`,
content NIP-44 self-encrypted owner→owner.

Putting the device id in the **d-tag** is the load-bearing choice. The NIP-33
replacement key is `(community, kind, pubkey, d_tag)`
(`crates/buzz-db/src/lib.rs:5169-5181`), so four machines signing with one
account key get four independent heads. This is exactly what
`buzz:{community}:presence:{pubkey_hex}` (`crates/buzz-pubsub/src/presence.rs:19-25`)
cannot do — four machines write one key and clobber each other, which is why
presence is useless for this.

Payload (v1 — everything here has a named consumer in the rank function or the
settings panel; nothing else is carried):

```rust
pub struct NodeDescriptor {
    pub v: u8,                    // 1
    pub device_id: String,        // 32 lowercase hex, == d-tag suffix
    pub device_name: String,      // user-editable label
    pub app_version: String,
    pub os: String,               // std::env::consts::OS
    pub arch: String,             // std::env::consts::ARCH
    pub cpu_cores: u16,           // std::thread::available_parallelism
    pub ram_total_mb: u64,        // the ONE fact needing a dependency
    pub harnesses: Vec<HarnessFact>,   // projection of AcpRuntimeCatalogEntry
    pub workspaces: Vec<WorkspaceFact>,// repo coord + free bytes
    pub agents: Vec<AgentFact>,        // runnable agent pubkeys + readiness
    pub running_agents: u8,
    pub policy: HostPolicy,
    pub content_hash: String,     // of everything above; drives republish + clone guard
}

pub struct HarnessFact {
    pub id: String,                     // AcpRuntimeCatalogEntry.id
    pub availability: String,           // AcpAvailabilityStatus discriminant
    pub auth: String,                   // AuthStatus discriminant
    pub credential: Option<CredentialFact>, // { kind, plan, account } — commit 79a55b9
}

pub struct WorkspaceFact { pub repo: String, pub free_mb: u64 }   // "30617:<owner>:<d>"

/// `requirements` is `Vec<Requirement>` from readiness.rs:295 — not a flattened
/// slug list. The unmet detail is exactly what the "no machine can run this"
/// message needs to name.
pub struct AgentFact { pub pubkey: String, pub ready: bool, pub requirements: Vec<String> }

/// One record per machine, published identically to every connected relay.
pub struct HostPolicy { pub schedulable: bool, pub max_concurrent_agents: u8 }
```

**Cadence.** Republish on `content_hash` change, plus a 60s floor, plus
immediately on wake. There is **no ephemeral heartbeat kind.** At four machines a
60s addressable republish is ~4 Postgres writes per minute — a rounding error —
and the history problem is solved by adding 30180 to the
`hard_delete_superseded` branch in `replace_parameterized_event`
(`crates/buzz-db/src/lib.rs:5210-5216`), the same mechanism kind:30003 uses. No
migration is needed: migration 0019's DELETE + trigger existed to backfill
history accumulated by an older relay, and a brand-new kind has no history.

**Liveness is local receive time, never `created_at`.** `handle_ephemeral_event`
(`crates/buzz-relay/src/handlers/event.rs:795-906`) verifies signature and
membership and imposes no timestamp bound; the ±5-minute window at `event.rs:984`
is special-cased to `KIND_AGENT_OBSERVER_FRAME`. So `created_at` is a remote
machine's wall clock. A Mac mini 90s slow because it just woke and NTP has not
resynced would be permanently labelled asleep; a machine 90s fast would look
alive for 90s after death. `created_at` is used for NIP-33 ordering and for
nothing else. Freshness window: 180s of local receive time (3× the 60s floor,
matching `PRESENCE_TTL_SECS`=180 on a 60s beat and
`REGISTRY_EXPIRY_MULTIPLIER`=3 at `crates/buzz-relay-mesh/src/registry.rs:21`).

**Privacy.** Content is NIP-44 self-encrypted (repo names, harness account
emails, machine names), and 30180 goes in `AUTHOR_ONLY_KINDS`
(`crates/buzz-core/src/kind.rs:129`, next to `KIND_PRIVATE_MANAGED_AGENT`).
Deliberately **not** in `P_GATED_KINDS`: `p_gated_filters_authorized`
(`crates/buzz-relay/src/handlers/req.rs:1058-1092`) closes any filter that could
match a p-gated kind unless `#p == self`, which is CLAUDE.md gotcha 3 and would
403 fleet queries. Implementation note: adding a kind to `AUTHOR_ONLY_KINDS`
trips the storage-level tripwire in
`crates/buzz-search/tests/fts_integration.rs::author_only_kinds_are_storage_level_unsearchable`
and must be reconciled with the search allowlist — `migrations/0008_fresh_install_search_allowlist.sql:16`
is a positive allowlist (safe automatically) but `schema/schema.sql:224` is still
a denylist, so the two must be updated together. Verify the current contents of
both before budgeting this; `AUTHOR_ONLY_KINDS` is only
`[KIND_EVENT_REMINDER, KIND_PUSH_LEASE]` today
(`crates/buzz-core/src/kind.rs:120`), so the reconciliation is small.

**Reset.** The frontend fleet store is a module-level singleton and must register
`resetFleetStore()` in `resetCommunityState()`
(`desktop/src/features/communities/useCommunityInit.ts`). The Rust publisher must
*also* be explicitly torn down and rebound on community switch — React key-based
remounting clears React state, not a live tokio task. Mirror the arrival-scope
guard in `mesh_llm/coordinator.rs`. `HostPolicy` is one per-machine record
published identically to every connected relay; it is not per-community. Per-
community policy would turn one Schedulable toggle into N the user must hand-sync.

### UI: Settings → "Your machines"

Four cards. Name (editable), OS/arch, "this machine" badge, a state dot
(Ready / Busy `2 of 3 agents` / Asleep `last seen 14 min ago`), harness chips
that finally make the commit-79a55b9 credential answer fleet-wide
(`claude-code ✓ Max · mfethe1@gmail.com`, `codex ⚠ not logged in`), a repo line,
and the two controls that matter: a **Schedulable** toggle and a **Max concurrent
agents** stepper. That toggle alone converts "four machines clashing" into "the
Air only helps when I say so" and is worth shipping before any scheduler exists.

---

## Work claiming and leases

### The decision: exclusion at the connection, not at the turn

The relay enforces **at most one authenticated agent connection per
`(community_id, agent_pubkey)`**. A second AUTH for a pubkey whose
`agent_owner_pubkey` is set (`crates/buzz-db/src/user.rs:354` `is_agent_owner`
— i.e. this pubkey is a managed agent, not a human) is rejected with
`restricted: agent already connected from another session`. The incumbent wins;
the challenger backs off.

Why this and not a per-turn lease with generations and a Postgres claims table:

1. **The desktop is not in the turn path.** `spawn_agent_child`
   (`managed_agents/runtime.rs:466`) launches buzz-acp, which independently
   subscribes and decides to answer. There is no turn-start hook in the desktop
   and no admission callback from the harness; the only desktop→harness channel
   is the observer *control* lane (`crates/buzz-acp/src/lib.rs:1068`), which
   handles `cancel_turn` and `switch_model`, silently ignores unknown payloads
   (`:1119`), and round-trips through the relay. A veto arriving after the turn
   starts is a cancel, not placement. Connection-level election, by contrast,
   happens at spawn time, and the desktop owns spawn.
2. **TCP death beats a lease clock.** `heartbeat_loop`
   (`crates/buzz-relay/src/connection.rs:424-455`) already detects a dead peer in
   ≤90s with zero new code, and tightening it to a 15s interval with 2 missed
   pongs gives ≤30s — better than the 45s TTL a lease would carry, with no renew
   traffic and no clock arithmetic.
3. **A lease's only advantage is owner-death recovery, and a naive lease design
   loses it anyway.** A losing machine that discards its batch has nothing to
   retry with; buzz-acp reads are on-demand and the WS subscription is live-tail,
   so a past trigger is never re-delivered. Connection-level election has real
   recovery: when the incumbent's socket dies the slot frees, a backed-off machine
   connects, and its subscription picks up from `since` on the live channel — the
   trigger events are still durably in the relay.
4. **No new kind, no new table, no per-event fence.** A publish fence in
   `handlers/ingest.rs` would add a Postgres round-trip on a shared hot path
   paid by every tenant of the Block relay deployment, to protect one home fleet.

**Rejected alternative — NIP-33 as a mutex.** `replace_parameterized_event`
(`crates/buzz-db/src/lib.rs:5134-5265`) puts pubkey *in* the replacement key, so
four machines signing with the account key never collide; and where they do
collide the resolution is `created_at` last-writer-wins (`:5247-5265`) — a steal,
not a mutex. The `"duplicate:"` → exit-5 signal
(`crates/buzz-relay/src/handlers/ingest.rs:2971`,
`crates/buzz-cli/src/commands/mod.rs:88-95`) names no winner and carries no
generation.

**Authorization.** The rule keys on the *agent pubkey being authenticated*, which
is the connection's own proven identity. There is no user-supplied work key and
therefore no cross-tenant squat: a community member cannot deny another user's
agent by asserting a string. This is the specific failure that sinks a
`work_key = sha256(agent_ref || channel_id)` design, where `agent_ref` is a hash
of public data that no relay table binds to any pubkey.

### Claim state machine

`Slot` is the relay-side state of `(community_id, agent_pubkey)`.
`Machine` is one desktop's view of one agent.

| Actor | State | Event | Next state | Side effect |
|---|---|---|---|---|
| Slot | `Free` | agent AUTH ok | `Held(conn, device)` | store conn_id; accept |
| Slot | `Held(c,d)` | agent AUTH ok from `c' ≠ c` | `Held(c,d)` | reject: `restricted: agent already connected from another session (<device_name>)` |
| Slot | `Held(c,d)` | conn `c` closed / 3 missed pongs | `Free` | broadcast nothing; next challenger wins |
| Slot | `Held(c,d)` | owner publishes `evict` control frame | `Free` | close `c` via `disconnect_pubkey` (`state.rs:371`) |
| Machine | `Idle` | agent becomes eligible (readiness Ready, schedulable, below cap) | `Waiting` | compute rank → delay; start timer |
| Machine | `Idle` | agent ineligible | `Idle` | — |
| Machine | `Waiting` | delay elapsed | `Connecting` | spawn harness / open agent WS + AUTH |
| Machine | `Waiting` | becomes ineligible (unplugged, harness logged out, policy off) | `Idle` | cancel timer |
| Machine | `Waiting` | peer descriptor change lowers our rank | `Waiting` | recompute delay; extend timer (never shorten below elapsed) |
| Machine | `Connecting` | AUTH ok | `Bound` | harness runs; publish descriptor with `running_agents+1` |
| Machine | `Connecting` | AUTH rejected (slot held) | `Backoff` | kill harness child; jittered timer 30–60s |
| Machine | `Connecting` | transport error | `Backoff` | jittered timer 5–15s |
| Machine | `Bound` | WS closed (network, sleep, crash) | `Backoff` | harness exits; timer 5–15s |
| Machine | `Bound` | user stops agent / policy off | `Idle` | stop harness; descriptor republish |
| Machine | `Bound` | becomes ineligible mid-run | `Bound` | finish current turn, then → `Idle` (never migrate a running turn) |
| Machine | `Backoff` | timer elapsed and still eligible | `Waiting` | recompute rank (peers may have changed) |
| Machine | `Backoff` | timer elapsed and ineligible | `Idle` | — |

Two properties worth stating. `Waiting → Waiting` never shortens an already-
running timer, so a descriptor arriving late cannot cause two machines to
converge on connecting simultaneously more often than they already would. And
`Bound → Backoff` on WS close means a machine that sleeps mid-turn does not hold
the slot: the relay frees it on pong timeout and a backed-off peer takes it.

### Failover semantics: restart, visibly

No harness in this codebase can adopt another process's ACP session — session
state lived in the dead machine's `AcpClient`. The taker re-reads the thread from
the relay (the durable truth) and runs a fresh turn. Streamed observer frames
from the dead machine are ephemeral and simply vanish. Duplicate *semantic* work
is possible in the window between the incumbent going dark and the relay noticing;
duplicate *published* replies are not, because the dead machine's socket is gone.
Surface it: `ActiveTurn` (`desktop/src/features/agents/activeAgentTurnsStore.ts`)
gains `deviceId`/`deviceName`, and `agentWorkingSignal.ts` renders
"Working on mac-mini-1". A visible restart beats a silent one.

---

## Placement policy

Placement produces an **ordering**, not an assignment. Each machine computes only
*its own* rank and converts it to a connect delay. Nothing requires the four
machines to agree; if two compute the same delay they race and the relay
resolves it. This is the `buzz-relay-mesh` law applied verbatim: *membership is a
hint, the fence is the arbiter* (`crates/buzz-relay-mesh/src/lib.rs:18-19`).

### Hard filter (a machine that fails any of these does not attempt)

1. `agent_readiness(agent, env)` is `Ready` (`managed_agents/readiness.rs:403`) —
   this is the existing pure per-(agent, machine) predicate; do not reimplement it.
2. A `ManagedAgentRecord` for this agent exists locally with its nsec in the
   keyring. Instances are non-portable today (`inbound.rs:408`), so this is the
   real candidate-set bound.
3. `policy.schedulable` is true.
4. `running_agents < policy.max_concurrent_agents`.
5. Per-agent concurrency below `effective_parallelism`
   (`managed_agents/parallelism.rs:41`) — reuse it; do not invent a second
   concurrency number.

### The rank (integers only, lexicographic)

```
rank(self, agent) = (
    0 if placement.pin == self.device_id else 1,   // T0 explicit user pin
    credential_rank(harness_for(agent)),           // T1 money
    0 if workspace_present(agent) else 1,          // T2 the checkout is here
    0 if harness_warm(agent) else 1,               // T3 a live buzz-acp child exists
    running_agents,                                // T4 spread
    hrw(agent_pubkey, device_id),                  // T5 deterministic tiebreak
)

credential_rank(h) = match h.credential {
    Subscription { plan: Some(_) } => 0,   // "Max", "Pro" — already paid for
    Subscription { plan: None }    => 1,
    _ if h.auth == NotApplicable   => 2,   // goose / buzz-agent: user's own key either way
    _ if h.auth == Unknown         => 3,
    ApiKey { .. }                  => 4,   // metered — real money per token
}

hrw(a, d) = u64::from_be_bytes(sha256(a || d)[..8])

// self.rank vs every peer's rank, computed from their fresh (<180s by local
// receive time) descriptors:
position   = count(peers whose rank < self.rank)
delay_ms   = position * 2000        // 0s, 2s, 4s, 6s
```

**No floats.** No weighted sum, no headroom term, no EWMA, no bucketing, no
thermal, no RTT, no battery. At four machines the tiers resolve essentially every
decision, and a five-term weighted score calibrated to a 0.114 power gap is a
scheduler for a datacenter. Free RAM does not enter v1: it is in the descriptor
for the settings panel and as future headroom, not in the rank.

**Credential rank is above every resource signal** and below only an explicit
pin. An ambient `ANTHROPIC_API_KEY` on one machine silently redirecting a Claude
Code agent from a paid subscription to per-token billing is the exact failure
commits 47f8ceb and 79a55b9 exist to surface; a scheduler that picks that machine
because it has more free RAM would be actively worse than no scheduler.

**Never silently fall back.** If no machine passes the hard filter, nothing runs,
and the failure is loud: one new `Requirement::NoEligibleNode { unmet, closest }`
variant on the existing enum (`readiness.rs:295`), which reuses the shipped
nudge-card and Doctor routing. "It ran, but on the wrong machine with the wrong
billing and no checkout" is precisely the failure being escaped. Machines with no
`PlacementPolicy` behave exactly as today (delay 0, always attempt), so existing
agents are unchanged byte-for-byte.

**Anti-thrash.** Placement is evaluated at connect time only. A `Bound` machine
is never preempted by a better-ranked peer arriving; it keeps the slot until its
socket dies or the user stops it. That single rule removes the need for incumbent
bonuses, decay schedules, and score bucketing.

**Policy lives on the definition.** `PlacementPolicy { pin: Option<String> }` on
`AgentDefinition` (`managed_agents/types.rs:16`), replicating for free over
kind:30175. Not on `ManagedAgentRecord` and not in the kind:30177 projection:
30177's d-tag is the agent's own pubkey (`managed_agents/agent_events.rs:113-124`)
and each machine mints a fresh `Keys::generate()` (`commands/agents.rs:624`), so
the four coordinates never see each other and the field would achieve nothing.

**Rejected: a constraint DSL.** `requires: ["harness:claude-code@authenticated",
"ram>=32G", "repo:block/buzz"]` with `#[serde(other)]` fail-closed parsing is a
scheduler constraint language for a home network. `agent_readiness()` already
answers can-this-run-here locally with typed unmet requirements; `pin` plus that
predicate is the whole requirement. `prefer_nodes`, `pin_fallback`,
`allow_metered`, and `quiet_hours` are all cut.

---

## Tailscale: what it is and is not for

**Finding: Tailscale carries nothing in this design, and neither does iroh.**

Walk the four traffic classes against what already ships:

| Traffic | Needs a peer link? | Why not |
|---|---|---|
| Node descriptors | No | kind 30180 over the relay WS every machine already holds |
| Claim / exclusion | **Cannot use one** | a P2P link cannot arbitrate mutual exclusion; only a single serialization point can, and that is the relay |
| Log / stream tailing | No | already works cross-machine via `KIND_AGENT_OBSERVER_FRAME`, NIP-44, `#p` = owner (`desktop/src/shared/api/observerRelay.ts`) |
| Bulk artifacts (code) | No | git. Agents already inherit the user's git credentials (`runtime.rs` has no `env_clear`) and push to the real origin |

So there is nothing left for an overlay to carry. Tailscale is also redundant
with iroh, which already ships in the desktop
(`desktop/src-tauri/src/mesh_llm/transport_policy.rs`) with direct QUIC plus
relay fallback and an endpoint allowlist — it already delivers the NAT traversal
a tailnet would provide, for the one workload (LLM serving) that genuinely needs
a peer link. Adding a *second* overlay would only add a way for the fleet to
disagree about reachability.

**What Tailscale actually earns: one config string, zero lines of code.** If the
user self-hosts the relay on an always-on Mac mini, MagicDNS makes it reachable
from all four machines while roaming with no port forwarding and no public IP:

```
BUZZ_RELAY_URL=ws://mini-1.tailXXXX.ts.net:3000
```

That is the entire integration.

**Explicitly not built:** no `tailnet.rs`, no `tailscale status --json` shell-out,
no `TailnetStatus`/`TailnetNode` types, no `require_tailnet` claim constraint, no
`tsnet` embedding. `tsnet` is Go — it needs a sidecar or FFI, adds ~10–15 MB, and
decisively registers a *fifth* userspace node rather than reporting the host's
own tailnet identity. `require_tailnet` defends only against a leaked nsec, and a
leaked nsec is game-over for the Buzz account anyway; its main practical effect
would be splitting your own fleet the day one machine's Tailscale is down.

**The honest cost of relay-centrism:** if the relay is self-hosted behind MagicDNS
and Tailscale is down, the whole fleet loses its control plane. State it plainly
rather than build a rarely-exercised second control plane. Mitigation is
deployment, not code: run the relay on the always-on Mac mini (not a laptop) and
expose it on a stable LAN address as well as MagicDNS, so a tailnet outage
degrades to LAN-only rather than total.

---

## Workspace binding

### Naming: reuse the NIP-34 coordinate

A workspace is named `30617:<owner-hex>:<repo-d>` — the existing
`KIND_GIT_REPO_ANNOUNCEMENT` coordinate (`crates/buzz-core/src/kind.rs:604-607`),
already the git ACL anchor via the `buzz-channel` tag
(`crates/buzz-relay/src/api/git/binding.rs`), already spoken by `buzz repos`. Do
not mint a new repo identifier.

It is **path-free by construction**, which is exactly what makes Windows and
macOS agree: the coordinate is identity, each machine resolves it to a local path
itself. A `WorkspaceFact { repo, free_mb }` in the descriptor is the presence
signal the T2 rank tier reads.

### Resolution: one durable worktree per (agent, repo)

```
<effective_repos_dir()>/<repo-d>/                    ← primary clone
<effective_repos_dir()>/.wt/<agent8>-<repo-d>/       ← this agent's worktree, branch buzz/<agent8>
```

`<agent8>` is the first 8 hex of the agent pubkey. `<repo-d>` is lowercased at
*announce* time, not hashed at resolve time.

**One worktree per agent, not per claim.** The ACP harness captures its working
directory once, process-wide: `crates/buzz-acp/src/lib.rs:2030` does
`cwd: std::env::current_dir()` into `PromptContext.cwd`, handed to every
`session/new` (`crates/buzz-acp/src/pool.rs:1010`). One buzz-acp process serves
all channels and turns for an (agent, relay) pair. A per-claim cwd is therefore
**not** a one-line change at `runtime.rs:583` — it requires per-session cwd
plumbed through `session_new_full`, a large cross-crate change. A durable
per-agent worktree needs only the spawn-site change, is created once and reused,
and matches what someone with four machines actually wants: a stable checkout per
agent, not a fresh one per task. It also eliminates the entire worktree GC policy
question.

Preserve the hardening being replaced: `default_agent_workdir()`
(`managed_agents/mod.rs:101-115`) explicitly rejects a symlinked `~/.buzz`
("Reject symlinks to prevent redirect attacks — `is_dir()` follows symlinks").
The resolved worktree path must carry the same `symlink_metadata` check.

### Two platform bugs that must be fixed for this to be honest

1. **`repos.rs:144-148`** — the `#[cfg(not(unix))]` arm ignores the configured
   `repos_dir` and just creates an empty `REPOS` directory. The Windows machine
   cannot point REPOS at a real checkout root, so it would enumerate no
   workspaces, permanently lose T2, and never win a repo-bound agent — silently,
   presenting as "the Windows box is never picked". Fix with a directory junction
   (`CreateSymbolicLinkW` with the DIRECTORY flag; junctions need no developer
   mode), or store the resolved root and skip the link indirection.
2. **`crates/buzz-acp/src/pool.rs:1362`** — `workspace_section` gates on
   `cwd != "/" && cwd.starts_with('/')`, so on Windows (`E:\...`) the
   `[Workspace]` grounding block is silently dropped from every system prompt;
   and when it does render it hardcodes `{cwd}/REPOS/`, which becomes a lie once
   cwd is a worktree. Both must be fixed together with the cwd change.

Note also `validate_repos_dir` (`repos.rs:37-40`) calls `canonicalize()`, which on
Windows yields `\\?\C:\...` extended-length paths that then flow into comparisons
and joins.

### Sync-back: origin first, and a channel-scoped pointer

The agent pushes `buzz/<agent8>` to the repo's **existing origin** (GitHub in the
normal case) using credentials it already inherits, then publishes a NIP-34
kind:1618 pull request. The relay git remote
(`crates/buzz-relay/src/api/git/transport.rs`) is an opt-in mirror, not the
default: it requires a kind:30617 announcement with a valid `buzz-channel` UUID
or every clone 404s, and it caps at `BUZZ_GIT_MAX_PACK_BYTES` 500 MB /
`BUZZ_GIT_MAX_REPO_BYTES` 1 GB (`crates/buzz-relay/src/config.rs:830-837`).

**A kind:1618 will not appear in a channel timeline.** All NIP-34 kinds are in
`is_global_only_kind` (`crates/buzz-relay/src/handlers/ingest.rs:568-577`) with
the explicit comment "git events use `a` tags (repo reference), not `h` tags
(channel scope)", and `ingest.rs:2237-2239` unconditionally sets
`channel_id = None`. A stray `h` tag is discarded. So the agent additionally
posts an ordinary channel message carrying the PR's `a` coordinate; the desktop
already renders entity links from `a` references.

**Git is an unfenced side channel.** The connection slot cannot reach
`git push` — a machine whose socket died still holds valid credentials for a few
seconds. Put the incumbency epoch in the branch name (`buzz/<agent8>-<epoch>`,
epoch incremented locally per `Idle→Bound` transition) so a woken stale machine
physically cannot force-push over its successor's branch. The orphan surfaces as
"abandoned work on `<device_name>`" rather than as silent data loss.

---

## New event kinds

Only integers verified free in `crates/buzz-core/src/kind.rs` (used in-range
today: 24134/24200/24242/24243/24810; 30174–30178; 43001–43006).

| Int | Name | Class | Tags | Purpose | Phase |
|---|---|---|---|---|---|
| 30180 | `KIND_NODE_DESCRIPTOR` | addressable (30000–39999) | `d = node:<32 hex>` | One machine's identity, harness inventory with billing credential, workspace inventory, runnable agents, and host policy. NIP-44 self-encrypted; `AUTHOR_ONLY_KINDS`; `is_global_only_kind`; `Scope::UsersWrite` in `required_scope_for_kind` (`ingest.rs:345`); `hard_delete_superseded` in `replace_parameterized_event` (`buzz-db/src/lib.rs:5210`). | 2 |
| 43101 | `KIND_AGENT_TURN_CLAIM` | regular | `h` (channel), `agent` (pubkey), `work` (hex), `gen` | **Reserved, not implemented.** Per-turn claim for cross-machine parallelism within one agent. Only built if Phase 4's trigger fires. | 4 (contingent) |
| 43102 | `KIND_AGENT_TURN_CLAIM_RELEASE` | regular | `work`, `gen`, `disposition` | **Reserved, not implemented.** Terminal release of 43101. | 4 (contingent) |

The 43100 block is chosen over extending 43001–43006 because that job block is
already rendered in the desktop timeline
(`desktop/src/features/messages/lib/formatTimelineMessages.ts:55`) and must not
be repurposed.

Kinds NOT added, and why:

- **No ephemeral node heartbeat (24310).** Four machines at a 60s addressable
  republish is ~4 writes/min. The ephemeral kind buys sub-minute load data that
  the integer rank function does not read. It also brings `boot_id`, `seq`,
  `descriptor_id`, a descriptor-staleness pointer, and a monotonic-vs-wallclock
  suspend detector — all of which become unnecessary once liveness is
  receive-time-based.
- **No claim kind in v1.** The connection slot is the exclusion mechanism and it
  needs no event.
- **No node-status kind separate from the descriptor.** One kind, one head.

---

## Failure modes

| Scenario | Detection | Mitigation | Blast radius |
|---|---|---|---|
| MacBook Air lid closes while `Bound` mid-turn | Relay pong timeout, ≤90s today (≤30s if tightened to 15s/2-missed) | Slot frees; a backed-off machine connects and re-reads the thread from the relay. Turn restarts, does not resume — no harness can adopt another's ACP session. | One turn restarted, visibly ("restarted on mini-1") |
| Machine wakes after 6h with a frozen fleet view | Descriptor local-receive-time age > 180s | Peers are simply stale and self-heal on the next inbound descriptor; our own rank is recomputed on the next `Backoff → Waiting`. No suspend detector needed. | One stale ranking decision |
| Two machines compute the same delay and race to AUTH | Relay: second AUTH sees `Held` | Loser is rejected with a NOTICE naming the incumbent, kills its harness child, and backs off 30–60s jittered. This is the designed common case, not an error. | One wasted process spawn |
| Clock skew (mini 90s slow after wake) | N/A — structurally avoided | Freshness is local receive time; `created_at` is used only for NIP-33 ordering. Without this the mini is permanently labelled asleep. | None |
| Time Machine restore / VM clone duplicates `device.json` | A descriptor arrives at our own coordinate with a `content_hash` we did not publish | Rotate to a fresh `device_id`, publish kind:5 for the old coordinate, log it. Catches convergence by any route, not just backup restore. | Would otherwise be silent and permanent |
| Two machines republish in the same wall-clock second | N/A — structurally avoided | The replacement key is `(community, kind, pubkey, d_tag)` and `d_tag` contains `device_id`, so two machines never contend on one coordinate. This is why the device id must be in the d-tag, not the content. | None |
| A node lies about capacity or credentials | Not detected | Accepted explicitly. All four machines hold the same account nsec; no signature could distinguish an honest node from a lying one. Damage is bounded because the *slot* is keyed on the authenticated agent pubkey, so a liar can take a turn but cannot cause two. | One misplaced agent |
| Ambient `ANTHROPIC_API_KEY` on one machine | `AuthCredential::ApiKey { source }` in that machine's descriptor | `credential_rank` 4 — that machine ranks last and connects 6s late; a Subscription machine always wins. Descriptor names the shadowing env var, so the cause is visible. | Prevented, was: silent per-token billing |
| Windows machine never picked for repo work | Silent today | Fix `repos.rs:144-148` (junction) and `pool.rs:1362` (POSIX gate) in Phase 3. Until then, Windows publishes an empty `workspaces` list, which is at least *honest* rather than a false claim. | One of four machines idle |
| No machine can run the agent (only Xcode box is off) | Hard filter yields zero candidates on every machine | `Requirement::NoEligibleNode { unmet, closest }` via the existing nudge card. Never a silent local fallback. | Agent does not run, loudly |
| Pinned machine is asleep | Same as above | Only that machine passes T0, so nobody connects. Rendered as "Ada is pinned to mac-mini-2, which is offline." A pin is never silently violated. | Agent does not run, visibly |
| Relay unreachable for all four machines | AUTH fails everywhere | Nothing runs. This is correct: an offline claim would have to be reconciled on reconnect, and reconciliation *is* the double-execution problem. State it as a known property. | Total, and correct |
| Self-hosted relay behind MagicDNS, tailnet down | Same as above | Deployment mitigation only: run the relay on the always-on mini and expose a LAN address alongside MagicDNS. Do not build a P2P fallback control plane. | Total |
| Stale machine has not seen a pin change and connects first | Not detected | The slot is still the arbiter — it wins and runs. Bounded, single-agent policy violation, self-healing when kind:30175 replication lands (already the shipped path). | One agent on the wrong machine until restart |
| Community switch leaks the old fleet or keeps publishing | Manual/test | `resetFleetStore()` in `resetCommunityState()` **and** explicit Rust-side teardown of the publisher task — React remounting does not stop a tokio task. | Stale panel + writes to the wrong relay |
| Adding 30180 to `AUTHOR_ONLY_KINDS` trips the FTS tripwire test | `crates/buzz-search/tests/fts_integration.rs` | Reconcile the `schema/schema.sql:224` denylist against `migrations/0008_fresh_install_search_allowlist.sql:16`. Budget it; it is not free. | CI red until fixed |

---

## What we deliberately are NOT building

**A peer-to-peer transport of any kind.** Not Tailscale in the data path, not
iroh for agent traffic, not `buzz-relay-mesh` participation. `buzz-relay-mesh` is
structurally closed to desktops on five independent counts: it is not a
dependency of `desktop/src-tauri/Cargo.toml`; its `ReadyRegistry` is raw
`deadpool_redis` SET/SCAN (`crates/buzz-relay-mesh/src/registry.rs:160-274`) with
no Nostr or HTTP proxy; membership is fail-closed against the server-only
`BUZZ_RELAY_PRIVATE_KEY` (`crates/buzz-relay/src/mesh_boot.rs:445-446`);
`accept_loop` drops unknown RuntimeIds (`runtime.rs:286-292`); and its crate law
forbids the mesh from deciding ownership at all (`lib.rs:18-19`). Its
`capabilities: Vec<String>` is a hardcoded three-string protocol list
(`mesh_boot.rs:371-377`) read by exactly one line and dropped from `/_mesh` — a
dead field, not a foundation.

**A Postgres claims table with generations and a per-event publish fence.** The
fence would add a Postgres round-trip in `handlers/ingest.rs` on a shared hot
path, paid by every tenant of the Block relay deployment, to protect one home
fleet — and it is opt-in by tag presence, so it silences a zombie only on the
publish paths someone remembered to thread the tag through. The connection slot
achieves the same exclusion with zero new state.

**A weighted continuous placement score.** No
`0.35·headroom + 0.25·(1−occupancy) + 0.15·power + ...`, no EWMA smoothing, no
0.02 bucketing, no incumbent bonus with a decay schedule, no rendezvous hashing
as a "three-birds" mechanism. The candidate set is at most four and usually one.
Worse, a self-relative proximity term (`1.0 if machine_id == me`) makes the score
*systematically* divergent across machines rather than transiently — every
machine would persistently believe it is the winner.

**Thermal state, RTT/proximity, battery, and GPU/VRAM in v1.** No cross-platform
thermal probe exists in the tree. Four machines on one tailnet are all sub-5ms.
Battery is not provided by `sysinfo` and would need a second crate with IOKit/WMI
baggage — and `allow_on_battery` was a policy knob nothing implemented. Three of
the four machines are Apple Silicon with unified memory, so RAM already tells the
GPU story.

**A constraint DSL.** `requires: ["harness:claude-code@authenticated", "ram>=32G"]`
with fail-closed `Unknown` parsing is a scheduler constraint language for a home
network. `agent_readiness()` already answers can-this-run-here with typed unmet
requirements.

**An ephemeral heartbeat kind, and everything it drags in.** `boot_id`, `seq`,
`descriptor_id`, `FleetNode.descriptorStale`, and the monotonic-vs-wallclock
suspend detector all exist to serve sub-minute telemetry the rank function does
not read.

**Per-claim worktrees, a `WorktreeGuard`, and a GC sweep.** Structurally
impossible without a large buzz-acp change (`lib.rs:2030` captures cwd once), and
unnecessary: one durable worktree per (agent, repo) is created once and never
garbage.

**Windows reserved-name validation and hashed path slugs.** `CON`/`PRN`/`AUX`/
`COM1-9` validators and `<lowercase(d)>-<hex8(sha256(...))>` directory names
defend against the user announcing both `Buzz` and `buzz` on their own relay.
Lowercase the d-tag once at announce time and keep directories readable.

**`workspace` on `ManagedAgentRecord` / in the kind:30177 projection.** 30177's
d-tag is the agent's own pubkey (`agent_events.rs:113-124`) and each machine
mints a distinct one, so four coordinates never see each other's field. Widening
a deliberately opt-IN allowlist for zero benefit.

**A tailnet module.** `TailnetStatus`, `TailnetNode`, `TailnetClaimPolicy`,
`require_tailnet`, and a `tailscale status --json` shell-out — all cut. See the
Tailscale section.

**`NodeState::Draining`, `quiet_hours`, `prefer_nodes`, `pin_fallback`,
`allow_metered`, `mesh_owner_id`, `os_version`, `cpu_model`, `disk_total_mb`.**
No consumer named for any of them.

---

## Implementation phases

### Phase 1 — Stop the clash (ships alone, useful alone)

**Goal.** Exactly one machine runs any given agent *identity*, chosen by boot
order. Everything else in this document is placement *quality*; this is
correctness.

**Scope, stated plainly.** This rule keys on the agent pubkey, so it fires only
when two machines present the same key — State B. On a State A fleet (which is
every fleet today, per the identity discussion above) it is correct but inert.
It is worth shipping first anyway: it is the invariant that must already hold
before identity mobility can be offered at all, and it is cheap. It is not, on
its own, a fix for four machines answering under four different keys.

**Touches.**
- `crates/buzz-relay/src/handlers/auth.rs` (or wherever `set_authenticated_pubkey`
  is called): after a successful NIP-42 AUTH, if
  `is_agent_owner(community, pubkey)` (`crates/buzz-db/src/user.rs:354`) shows the
  pubkey is a managed agent, check `ConnectionManager` for an existing
  authenticated connection for that pubkey in this community
  (`crates/buzz-relay/src/state.rs:553`, `:570`). If one exists, close with
  `restricted: agent already connected from another session`.
- `crates/buzz-relay/src/state.rs` — a `find_agent_conn(community, pubkey)`
  helper; the storage already exists.
- `crates/buzz-relay/src/connection.rs:434` — tighten the heartbeat interval from
  30s to 15s so slot recovery is ≤30s rather than ≤90s. One constant.
- `crates/buzz-acp/src/lib.rs` — on an AUTH rejection carrying the
  `agent already connected` prefix, exit cleanly with a distinct code instead of
  reconnect-looping.
- `desktop/src-tauri/src/managed_agents/runtime_commands.rs` — treat that exit
  code as `Backoff`: do not restart for 30–60s jittered, and surface
  "Ada is running on another machine" in the agents list rather than a red error.
- Gate the whole rule behind a relay config flag (default on for single-tenant,
  reviewed before enabling on the shared Block deployment).

**Done when.** Import the same agent nsec onto two machines via the existing
snapshot envelope, start both, post one non-mention message in a channel the
agent watches: exactly one reply. Kill the winner's desktop process; within 30s
the other machine connects and answers the next message. `just ci` green;
integration test in `crates/buzz-test-client/tests/e2e_relay.rs` asserting the
second AUTH is rejected and that closing the first frees the slot.

### Phase 2 — Know your machines

**Goal.** A "Your machines" panel and a per-machine Schedulable toggle. No
placement logic yet.

**Touches.**
- `crates/buzz-core/src/kind.rs` — `KIND_NODE_DESCRIPTOR = 30180`; add to
  `ALL_KINDS` and `AUTHOR_ONLY_KINDS`.
- `crates/buzz-core/src/node_descriptor.rs` (new, modeled on
  `private_managed_agent.rs`) — types, NIP-44 self-encrypt/decrypt, d-tag
  build/parse validating `node:<32 hex>`.
- `crates/buzz-relay/src/handlers/ingest.rs` — `Scope::UsersWrite` in
  `required_scope_for_kind` (`:345`), add to `is_global_only_kind` (`:529`).
- `crates/buzz-db/src/lib.rs:5210` — add 30180 to `hard_delete_superseded`.
- `schema/schema.sql:224` + the FTS tripwire test — reconcile the search
  allowlist and denylist so both carry the new kind.
- `desktop/src-tauri/src/device/identity.rs` (new, not feature-gated) —
  `device.json` load-or-generate, self-coordinate clone guard, rotate + kind:5.
- `desktop/src-tauri/src/device/facts.rs` (new) — os/arch from
  `std::env::consts`, cores from `std::thread::available_parallelism`, RAM total
  from `sysinfo` (the only new dependency, and only for that one field);
  harness projection from `discover_acp_runtimes_from`
  (`managed_agents/discovery.rs:1474`); workspace enumeration from
  `effective_repos_dir()` (`managed_agents/repos.rs:186`), cached at launch plus
  explicit refresh, never on a timer (process spawn is expensive on Windows).
- `desktop/src-tauri/src/device/announce.rs` (new) — publish on content-hash
  change + 60s floor + on wake; explicit teardown and rebind on community switch.
- `desktop/src/features/fleet/fleetStore.ts` (new) — subscribe
  `{kinds:[30180], authors:[self]}`, decrypt, derive liveness from **local
  receive time**; register `resetFleetStore()` in `resetCommunityState()`.
- `desktop/src/features/fleet/ui/YourMachinesSection.tsx` (new) — rem-based text
  tokens only (CLAUDE.md § text sizing).
- `docs/nips/NIP-ND.md` (new) — kind number, d-tag layout, payload schema,
  cadence, author-only read rule.

**Done when.** All four machines appear in the panel with correct harness chips
including which credential each would bill; toggling Schedulable off on the Air
persists and is visible from the Windows box within 60s; a community switch
leaves no stale rows and no stale publisher.

### Phase 3 — Place by rank

**Goal.** The best-placed machine wins the slot, not the first to boot.

**Touches.**
- `desktop/src-tauri/src/managed_agents/placement.rs` (new) — the hard filter and
  the six-tuple rank; pure, no I/O, no clock, unit-tested like
  `agent_readiness`.
- `managed_agents/types.rs` — `PlacementPolicy { pin: Option<String> }` on
  `AgentDefinition`; include in the kind:30175 projection and content hash.
- `managed_agents/readiness.rs:295` — `Requirement::NoEligibleNode`.
- `managed_agents/runtime_commands.rs:459` — apply `delay_ms` before
  `start_pair`; recompute on descriptor change while `Waiting`.
- `managed_agents/repos.rs:144-148` — Windows junction so the Windows machine can
  actually hold workspaces.
- `crates/buzz-acp/src/pool.rs:1362` — fix the POSIX gate in `workspace_section`
  and stop hardcoding `{cwd}/REPOS/`.
- `managed_agents/runtime.rs:583` — resolve a per-agent worktree cwd when the
  agent has a bound workspace, preserving the `symlink_metadata` rejection from
  `mod.rs:101-115`; fall back to `default_agent_workdir()` otherwise.
- `desktop/src/features/agents/ui/whereToRunIntent.ts` — `runOn: "auto"` as a
  real discriminated variant (today `"local" | string` collapses to `string`),
  and render `WhereToRunSection` unconditionally — it currently returns `null`
  when no backend provider binary is on PATH (`WhereToRunSection.tsx:86`), which
  is the normal case, so a new default mode would otherwise be invisible.
- `crates/buzz-cli/src/commands/agents.rs` — `buzz agents placement explain
  --agent <pubkey>`, printing the full candidate table and each machine's
  computed delay without connecting anything. This is the single feature that
  makes the ordering trustworthy.

**Done when.** With Claude Code logged into a subscription on mini-1 and only an
`ANTHROPIC_API_KEY` on mini-2, mini-1 wins every time and `placement explain`
shows credential rank as the deciding tier. Unplug mini-1: mini-2 takes the slot
within 30s. Windows enumerates its repos and wins a repo-bound agent when it is
the only machine with the checkout.

### Phase 4 (contingent) — Per-turn claims

**Do not build this unless a specific need appears:** one agent needs to work
concurrently on several threads across several machines, and per-agent placement
is measurably leaving three machines idle. Until then, parallelism across
*different* agents is what the slot model already gives, and it is enough for
four personal machines.

If the trigger fires: implement 43101/43102 against a Postgres
`agent_turn_claims` table modeled on the `lease_owner`/`lease_generation`/
`lease_until` trio in `crates/buzz-db/src/deletion.rs:940-1060` and the
generation-CAS upsert in `crates/buzz-db/src/push.rs:484-510`. Key on
`(community_id, agent_pubkey, trigger_event_id)` — the agent pubkey is the
connection's own proven identity and the trigger event id is the one identifier
all machines demonstrably see identically. The claim call goes in
`crates/buzz-acp/src/queue.rs` at `flush_next()` (`:260`), which means
restructuring `dispatch_pending` (`crates/buzz-acp/src/lib.rs:3532`) — a
synchronous loop holding `&mut` on both pool and queue — to allow an await. A
loser must **retain** its batch and retry on lease expiry; a design where the
loser discards has no taker when the owner dies and converts "two machines answer"
into "sometimes zero machines answer".

---

## Open questions

1. **State A or State B?** Do you want four sibling agents (one per machine,
   distinct pubkeys, distinct avatars) or one agent that moves between machines?
   This design assumes **one identity** — and there is currently **no way to
   produce that state**. As established above, the snapshot surface excludes
   identity by construction and import always mints a fresh keypair, so a fleet
   is necessarily in State A no matter what the operator does by hand. Choosing
   "one identity" therefore selects a feature that does not exist yet, not a
   configuration.

   Making it exist means a deliberate identity-export path carrying
   `private_key_nsec` + `auth_tag`. That is a security decision before it is an
   engineering one: those two fields are on the never-serialized list on purpose,
   an exported nsec is a bearer credential for an identity the relay bills and
   trusts, and today's locked-envelope refusal semantics assume the importing
   machine *already* holds the key. No existing NIP in `docs/nips/` covers
   identity migration — the closest neighbours are NIP-AE (snapshot envelope,
   which deliberately excludes identity) and NIP-OA (agent→owner delegation).
   Designing one is its own project.

   Until then Phase 1 is a *precondition*: it makes one-identity-many-machines
   safe to allow, and is inert on a State A fleet because there is no shared
   pubkey to exclude on. Anyone reading this doc as "Phase 1 stops my four
   machines from double-answering" will be disappointed unless their agents
   genuinely share a key.

2. **Do you self-host the relay?** This is the only question that decides whether
   Tailscale earns anything. Self-hosted on a Mac mini → MagicDNS in
   `BUZZ_RELAY_URL` is genuinely valuable and needs zero code. Hosted relay →
   Tailscale contributes nothing to this design.

3. **Is the shared Block relay in scope?** The Phase 1 connection rule is
   proposed behind a config flag. If this only ever runs against your own relay,
   the flag can default on and the review burden is small. If it lands on the
   shared staging deployment, the rule needs a look from whoever owns
   `squareup/block-coder-tf-stacks`.

4. **Is `sysinfo` acceptable for one field?** It is the only new dependency, and
   after `std::env::consts` and `std::thread::available_parallelism` it buys
   exactly `ram_total_mb` (plus disk-free if we want it). The alternative is ~200
   lines of per-platform code or dropping total RAM from the descriptor entirely.

5. **Should the slot be per (community, agent) or per (community, agent,
   channel)?** Per-agent is proposed: one machine owns an agent entirely. Per-
   channel would let two machines split one agent's channels, which is real
   parallelism, but it means the connection can no longer be the unit and pushes
   you straight to Phase 4.

6. **Heartbeat tightening.** Dropping the relay ping interval from 30s to 15s
   (`connection.rs:434`) improves slot recovery from ≤90s to ≤30s for everyone,
   at the cost of doubling ping traffic across all tenants. Acceptable, or make it
   configurable?

7. **Buzz-on-Buzz.** CLAUDE.md gotcha 6: `just desktop-tauri-fmt` fails inside
   git worktrees, so an agent provisioned into a worktree of the Buzz repo itself
   hits a pre-commit failure. Fix as part of Phase 3, or accept and document?
