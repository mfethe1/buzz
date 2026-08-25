# Buzz System: signed work coordination and CML

**Status:** implementation specification  
**Upstream base:** `block/buzz@bb5b9357a7c8`  
**Owner:** Lenny / Protelynx  
**Scope:** planner → worker → reviewer ↔ fixer coordination, canvas projection, agent liveness, git/runtime evidence, and portable export

## 1. Product fit

Buzz remains the pipe, not the brain. Agents plan and execute; Buzz stores signed facts, enforces channel access, projects a readable board, and preserves the evidence. This extends the existing product rather than introducing a second control plane:

- channel canvases already use `KIND_CANVAS = 40100` and are available through desktop and `buzz canvas get|set`;
- execution already uses `KIND_JOB_REQUEST..KIND_JOB_ERROR = 43001..43006`;
- presence already uses ephemeral `KIND_PRESENCE_UPDATE = 20001`, a relay-signed `KIND_PRESENCE_SNAPSHOT = 40902`, and a 180-second Redis TTL;
- private owner-scoped observer frames already use ephemeral `KIND_AGENT_OBSERVER_FRAME = 24200`;
- projects and git state already use NIP-34 plus `KIND_PROJECT = 30621`;
- approval and trace events already occupy the workflow range `46001..46031`.

The implementation MUST preserve the product contracts in `VISION.md`, `VISION_AGENT.md`, `VISION_PROJECTS.md`, `VISION_ACTIVITY.md`, `VISION_MESH.md`, and `VISION_REMOTE_AGENTS.md`: Nostr-first operations, protocol composition instead of runtime coupling, replaceable compute bodies, heartbeat-derived presence, and evidence visible at a glance.

## 2. Decisions

1. **Signed Nostr events are live authority.** A task state is a deterministic reduction of valid events, not mutable canvas JSON or a private HTTP row.
2. **CML is a portable snapshot.** `.cml` and fenced canvas blocks serialize the reduced state for humans, git, export, and interop; they never arbitrate concurrent writes.
3. **Reuse existing kinds.** Version-1 coordination is an extension of job events `43001..43006`, not another task event family.
4. **The canvas is a projection.** It may lag and be regenerated from events. Editing a task card emits a signed transition first and then refreshes the projection.
5. **Presence is derived.** `online|stale|offline` is calculated from signed heartbeat age plus active lease state. An agent cannot declare itself healthy indefinitely.
6. **Privacy is fail-closed.** Public task state uses a pseudonymous `host_id` and relative `worktree_alias`. Raw IPs, absolute paths, credentials, environment values, and substrate secrets are rejected from CML and signed task content.
7. **Review is adversarial and bounded.** A fresh reviewer evaluates acceptance evidence without the implementer's narrative. Reviewer → fixer may repeat at most three times; the fourth rejection moves the task to `blocked` and requires human intervention.
8. **Git is the implementation authority.** Claims include base/head SHA and branch. “Shipped” requires merged SHA, installed/runtime revision match, and observable behavior—not a self-report or green CI alone.

## 3. Architecture

```text
human / planner / worker / reviewer / fixer
                    │
                    │ signed events (NIP-01 over WS/HTTP bridge)
                    ▼
             Buzz relay + event store
                    │
       ┌────────────┼──────────────┐
       ▼            ▼              ▼
 deterministic   presence       branch / PR /
 task reducer    projection      workflow evidence
       │
       ├── CML canonical export (.cml)
       ├── channel canvas projection (```buzz-workstream-card)
       ├── desktop Workstream Board
       └── buzz CLI / ACP / MCP adapters
```

NATS and peer-relay remain optional execution adapters. They can dispatch a body or report host capacity, but a dispatch is not accepted until the assigned agent publishes the signed Buzz acknowledgment. Loss of NATS/peer-relay therefore cannot create a false durable state.

## 4. State machine

```text
proposed
  └─planner.plan──────────────► planned
       └─worker.claim─────────► claimed
            └─worker.start────► working
                 ├─worker.block────────────► blocked
                 └─worker.submit───────────► review
                       ├─reviewer.approve───► verified
                       └─reviewer.reject────► fixing
                              └─fixer.submit► review  (max 3 rounds)

verified ─integrator.merge────► integrated
integrated ─runtime.prove─────► shipped
any nonterminal ─cancel───────► cancelled
any nonterminal ─fork detected► conflicted
conflicted ─owner.resolve─────► selected predecessor state
review rejection after round 3► blocked
expired exclusive claim───────► planned (lease-expired transition)
```

### Transition authority

| Transition | Required actor | Required evidence |
|---|---|---|
| `planner.plan` | planner or owner | objective, acceptance criteria, repo/base SHA |
| `worker.claim` | assigned worker | unexpired exclusive lease, branch, worktree alias, host ID |
| `worker.start` | lease holder | current head SHA |
| `worker.submit` | lease holder | head SHA, test/build evidence handles |
| `reviewer.approve` | reviewer distinct from worker/fixer | acceptance verdict and evidence handles |
| `reviewer.reject` | reviewer distinct from worker/fixer | actionable findings, round ≤ 3 |
| `fixer.submit` | assigned fixer | new head SHA, finding dispositions |
| `integrator.merge` | authorized maintainer/integrator | merge SHA and green exact-head gates |
| `runtime.prove` | verifier distinct from worker/fixer | running revision plus observable behavior |

The reducer rejects unknown transitions, skipped states, actor-role conflicts, stale `prev` references, non-monotonic rounds, and transitions against an expired or foreign lease.

## 5. Event contract

All coordination events are ordinary signed Nostr events with channel scope.

### Common tags

```json
[
  ["h", "<channel-uuid>"],
  ["d", "<task-uuid>"],
  ["protocol", "buzz-cml", "1"],
  ["transition", "worker.submit"],
  ["status", "review"],
  ["role", "worker"],
  ["e", "<previous-transition-event-id>", "prev"],
  ["a", "30617:<repo-owner-pubkey>:<repo-id>", "repo"]
]
```

- `43001` creates the task/job and carries the initial CML payload.
- `43002` records claim/acceptance and lease metadata.
- `43003` records all nonterminal progress and state transitions.
- `43004` records a verified final execution result; it does not by itself mean integrated or shipped.
- `43005` records cancellation.
- `43006` records terminal execution error.

Every event references its immediate predecessor with `e:...:prev`. Forks are retained as evidence but the reducer selects no winner silently: conflicting successors place the task in `conflicted`. Only an authorized channel owner or designated resolver may exit that state by publishing a `43003` `owner.resolve` transition containing both `["e","<head-a>","fork_a"]` and `["e","<head-b>","fork_b"]`, plus `["e","<selected-head>","selected"]`. The reducer returns to the selected head's predecessor state and preserves the resolution event in the audit chain.

### Lease

```json
{
  "lease": {
    "id": "018f...",
    "holder": "<agent-pubkey-hex>",
    "issued_at": 1787673000,
    "expires_at": 1787673900
  }
}
```

Lease duration is bounded by relay policy. Renewal is a signed `43003` transition referencing the current head. A host heartbeat does not renew a task lease and a task lease does not imply host health.

## 6. CML v1

CML is canonical UTF-8 JSON with sorted object keys, two-space indentation, LF endings, and one final newline. Files use `.cml`. A channel canvas embeds the same object in a fenced block named `buzz-workstream-card` for compatibility with the existing Workstream Board work.

```json
{
  "acceptance": [
    {"id": "A1", "text": "Persisted state round-trips", "verified": false}
  ],
  "blockers": [],
  "evidence": [],
  "git": {
    "base_sha": "<40-hex>",
    "branch": "feat/example",
    "head_sha": "<40-hex-or-null>",
    "repo": "owner/repo",
    "worktree_alias": "buzz-example"
  },
  "id": "<task-uuid>",
  "lease": null,
  "objective": "One testable outcome",
  "priority": "P1",
  "protocol": "buzz-cml",
  "review": {"max_rounds": 3, "round": 0},
  "roles": {
    "fixer": null,
    "planner": "<pubkey-hex>",
    "reviewer": null,
    "worker": null
  },
  "runtime": {
    "host_id": null,
    "last_heartbeat_at": null,
    "presence": "offline",
    "ttl_seconds": 180
  },
  "status": "proposed",
  "title": "Example task",
  "updated_at": 1787673000,
  "version": 1
}
```

### Validation rules

- reject unknown top-level keys in v1 except under an explicit `extensions` object;
- UUIDs and 40/64-hex identifiers are lowercase canonical form;
- enums are closed and case-sensitive;
- acceptance IDs are unique and non-empty;
- `review.max_rounds` is exactly `3` in v1 and `round` is `0..3`;
- role pubkeys are distinct where separation of duty applies;
- `host_id` is derived as `h_` plus the first 16 lowercase hex characters of `HMAC-SHA256(host_secret, community_id || channel_id || agent_pubkey)`. `host_secret` is a locally generated 256-bit value persisted in the host keyring, never sent to the relay. This is stable for one agent/host/channel while preventing cross-channel correlation and making different agents on one host distinct;
- `worktree_alias` is a basename-like token, never absolute and never containing `..`;
- evidence values are content hashes, event IDs, commit SHAs, or authorized URLs—not inline logs or secrets;
- canonical serialize → parse → serialize is byte-identical.

## 7. Presence and privacy

### Derived status

Given `age = now - last_signed_heartbeat` and TTL `T = 180s`:

- `online`: age ≤ T and, for an active task, the heartbeat identity equals the lease holder;
- `stale`: T < age ≤ 2T while an unexpired lease exists;
- `offline`: no heartbeat, age > 2T, or explicit signed disconnect;
- `lease_expired`: task lease elapsed regardless of presence.

The board displays stale/offline states but never treats them as proof the substrate is dead. This matches the remote-agent contract: presence means conversational availability, not CPU/process telemetry.

### Privacy tiers

| Field | Default task/channel visibility | Private owner telemetry |
|---|---|---|
| agent pubkey / role | yes | yes |
| pseudonymous `host_id` | yes | yes |
| branch, SHA, worktree alias | yes | yes |
| raw hostname / IP | no | optional |
| absolute worktree path | no | optional |
| CPU/RAM/process list | no | optional |
| credentials/env/secrets | never | never |

Raw substrate telemetry, if enabled, travels only in encrypted owner-scoped observer frames and is never copied into CML, canvas, task events, screenshots, or logs. Enforcement is layered: `buzz-core` CML/task validators reject unsafe fields and references; `buzz-sdk` builders validate before signing; relay ingest validates signed task content again before persistence/fan-out; desktop/CLI canvas import treats invalid blocks as inert Markdown and never emits a transition. No adapter may bypass relay validation through a private mutable task API.

### Canvas history and projection

Canvas history stores raw Markdown versions, not task authority. Restoring an older canvas never rewinds signed task state: the reducer re-derives the current CML block from events. If restored Markdown contains a divergent block, Buzz preserves the free-form text, replaces the block with the event-derived projection, and surfaces a `canvas-stale` advisory. Any user edit to a task field emits and confirms a signed transition first; only then does optimistic-concurrency canvas save update the projection.

## 8. Existing-work reconciliation

### `origin/feat/task-system` at `a2bd15f8ee3f`

Live comparison against `upstream/main@bb5b9357a7c8` shows 31 commits behind, 4 commits ahead, 33 files, and 5,673 insertions. It must not be merged wholesale.

| Slice | Decision | Reason |
|---|---|---|
| `buzz-core/src/task.rs` domain vocabulary/tests | **salvage concepts selectively** | useful task fields and invariants, but live authority must become signed events |
| `buzz-db/src/task.rs`, SQL tables | **do not make authoritative** | mutable HTTP/DB task state conflicts with Nostr-first and creates dual truth |
| relay `/api/tasks` | **do not port as primary API** | agent operations belong in signed events and `buzz-cli` |
| mobile task sheet/models | **defer, then adapt to CML reducer** | valuable UX after the event contract is stable |
| thread summarization | **separate portable feature** | useful but not required for coordination correctness |

### `upstream/workstream-board/01..08`

The stack is valuable but currently 89 commits behind live upstream and 35 commits ahead at stage 8, touching 58 files with 5,597 insertions. Build a clean `integration/workstream-board-cml` from current upstream and port cited commits in order; never merge the raw head.

| Capability | Decision |
|---|---|
| canvas fenced-card parser and discovery | port first; extend parser to full CML v1 |
| active-turn projection | reuse as supplementary activity, not presence authority |
| PR status and wait sorting | port |
| presence timer retry | reconcile with current presence code and TTL reducer |
| agent status pills | port using derived status |
| blocker links and typed context references | port after core reducer |
| large replay screen changes | last; split if needed to preserve reviewability |

### Newly published upstream integration work (2026-08-25)

Fresh upstream discovery found four directly relevant stacks created or updated during this delivery window:

| Branch / head | Ahead / behind current main | Integration decision |
|---|---:|---|
| `duncan/canvas-version-history@d225a60494e9` | 1 / 3 | **Port before canvas projection.** It adds optimistic concurrency, history, restore, and CLI support across six files; this removes the need for CML to invent canvas conflict control. |
| `hayt/canvas-history-desktop@4441474789ce` | 1 / 3 | **Stack after relay history.** Reuse its conflict-checked save and history panel, then add CML-aware diff labels. |
| `larry/workflow-durable-message-delivery@700011f7c00d` | 10 / 3 | **Evaluate as dispatch transport.** Its revision-bound workflow execution and durable managed-agent inbox can replace bespoke delivery acknowledgment, while signed CML task events remain durable work authority. Port only after its migration/event-kind overlap is reconciled. |
| `duncan/nip-fi-verifier-contracts@009f9ac4bf5f` | 2 / 3 | **Security dependency candidate.** Its canonical assertion verifier may strengthen federated identity for cross-repo adopters, but is not required for CML v1 and must be reviewed separately. |

These branches are moving upstream work, not released APIs. Track exact SHAs, port selectively onto fresh current-main branches, and rerun negative controls; do not merge raw heads.

## 9. Block ecosystem leverage

- **Advanced Context Infrastructure (ACI):** import/export curated context references as CML evidence/notes; do not copy raw transcripts into signed task state. ACI's deterministic audits are candidates for reviewer evidence.
- **CoPlan:** map an approved plan/version to `planner.plan`; preserve CoPlan URL/version provenance and use Buzz for execution/review state rather than duplicating CoPlan editing.
- **Berd:** ship a Buzz System skill and handoff adapter so private Goose/ACP work can claim and update Buzz tasks. Berd already publishes a `buzz-handoff` skill.
- **Agent Task Queue:** use as a host-local capacity adapter below leases. Queue admission does not equal a Buzz claim; signed acknowledgment closes that gap.
- **Builderbot/Staged:** ingest pinned ACP plan and child-note aggregation as planner/evidence inputs; publish Buzz task events through a small adapter.
- **Trailblaze:** use recorded deterministic trails plus report URLs/hashes as UI acceptance evidence and installed-app runtime proof.
- **`wt`:** consume `wt list --porcelain` for worktree aliases and dirty/ahead/behind diagnostics while filtering absolute paths from public state.
- **`lhm`:** enforce local quality gates consistently, while Buzz records only exact-head gate results and hashes.
- **Ghost:** attach applicable design-guidance node IDs and review assertions to CML acceptance/evidence; the package remains repo-local.

Sources:
- https://github.com/block/advanced-context-infrastructure
- https://github.com/block/coplan
- https://github.com/block/berd
- https://github.com/block/agent-task-queue
- https://github.com/block/builderbot
- https://github.com/block/trailblaze
- https://github.com/block/wt
- https://github.com/block/lhm
- https://github.com/block/ghost

## 10. Delivery sequence

1. **CML core:** JSON schema, strict parser, canonical serializer, fixtures, negative controls, CLI import/export.
2. **Signed reducer:** builders/validators/reducer over kinds `43001..43006`, conflict and lease tests, privacy rejection tests.
3. **Canvas projection:** parse existing fenced cards, render CML, preserve free-form Markdown, deterministic regeneration.
4. **Workstream UI:** clean-port the board parser/discovery, then status, waits, evidence, git/runtime metadata, and references.
5. **Adapters:** Hermes/peer-relay/NATS, Berd/Goose/ACP, CoPlan, Agent Task Queue, Trailblaze evidence.
6. **Durable delivery reconciliation:** audit `larry/workflow-durable-message-delivery@700011f7c00d` for migration and event-kind overlap. If compatible, place its managed-agent inbox behind dispatch while retaining signed `43002` acknowledgment as the sole durable acceptance signal; otherwise document the incompatibility and retain NATS/peer-relay delivery without inventing another kind.
7. **Live E2E:** run `scripts/e2e-buzz-system.sh` against an isolated relay: create task → worker claim → worker submit → reviewer reject → fixer submit → reviewer approve → merge record → installed runtime proof. The script receives only channel/task IDs and expected fixture hashes, exits nonzero on any missing/invalid event, and verifies each reduced CML snapshot byte-for-byte.
8. **Adversarial gate:** forged role, stale lease, replay, forked transition, leaked absolute path/IP/secret, heartbeat expiry, reviewer=worker, fourth fix round.

## 11. Acceptance gates

- CML schema fixtures validate identically in Rust and TypeScript.
- Invalid, oversized, ambiguous, or privacy-leaking CML fails closed.
- Canonical round-trip is byte-identical.
- Event signatures and channel membership are checked by the normal relay path.
- The reducer is deterministic under event reordering and exposes conflicts rather than choosing silently.
- A forked transition produces `conflicted`; only an authorized signed `owner.resolve` referencing both heads and the selected head exits it.
- Presence changes to stale/offline under fake time without a fresh heartbeat.
- A reviewer cannot approve its own worker/fixer output.
- Round four cannot begin.
- Canvas free-form Markdown survives projection updates.
- Restoring a canvas version with stale CML does not change event-derived task state and emits a `canvas-stale` advisory.
- Dispatch-layer pending state is invisible to the reducer; only a valid signed `43002` claim changes durable acceptance state.
- A real Hermes worker on another fleet host acknowledges and completes a signed task.
- `scripts/e2e-buzz-system.sh` proves all eight lifecycle events, byte-identical expected CML snapshots, merge SHA, installed revision, and independently hashed observable output.
- The blind verifier receives only task UUID, channel, and pre-published expected hashes—never the worker narrative or environment.
- Before/after screenshots use an immutable baseline and objective pixel diff.
- Full repository gates pass on the exact integration head, or inherited failures are reproduced on the base and reported honestly.

## 12. Explicit non-goals for v1

- replacing NATS, peer-relay, ACP, MCP, CoPlan, or Agent Task Queue;
- publishing raw host telemetry or absolute filesystem paths;
- treating canvas text as a concurrency database;
- autonomously merging to protected branches without existing Buzz/git policy;
- unbounded reviewer/fixer loops;
- a new generic workflow engine—the existing Buzz workflow system remains the automation layer.
