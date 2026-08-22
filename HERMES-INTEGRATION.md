# Hermes ↔ Buzz — Integration Stack

**Branch:** `integration/hermes-agent-coordination` (fork `mfethe1/buzz`)
**Base:** `block/buzz@07456123` (current main at stack time)
**Built:** 2026-08-21 · owner: Hub/Lenny

## Why this branch exists

25 open PRs, **0 of 28 ever merged upstream**, while block/buzz merges insiders at a
**12h median**. Three of our PRs were closed as *superseded* — an insider fixed the same
defect faster. Racing insiders on small defect fixes is unwinnable.

Agent-to-agent coordination is the opposite: **nobody upstream is racing us there.**
This branch stops treating our work as 25 lottery tickets and assembles it into one
coherent artifact that demonstrates something upstream cannot casually reproduce.

## Architecture decision: adapter, not absorption

Hermes bot code does **not** get copied into Buzz. The boundary:

- **Hermes keeps:** Telegram/Discord transports, credentials, bot lifecycle.
- **Buzz keeps:** agent identity, task assignment, conversation state, relay events,
  acknowledgements, audit history.
- **Adapter maps:** Hermes agent identity → stable Buzz agent+device identity;
  inbound bot message → Buzz task/event; Buzz assignment/mention → Hermes agent turn;
  Hermes result → Buzz task events.

Protocol/sidecar boundary, not a code merge. Keeps license and secret surfaces clean.

## Stack layers (33 commits, tree clean, rustfmt green)

| Layer | PRs | Result |
|---|---|---|
| 1. Tooling / test reliability | 6239, 6278, 6279, 6286, 6355, 6220 | 7 commits, **0 conflicts** |
| 2. Identity | 6013, 6259, 6037 (+6235, 6077 deferred) | 15 commits, 4 deferred |
| 3. Reliable transport | 6118, 6090, 6170, 6365, 6126 | 9 commits, 1 deferred |
| 4. Assignment + tasks | 6060, 6425 | 2 commits, 4 deferred |
| 5. Hermes adapter | — | **not started** (needs stable contracts below) |

Method: replay each PR's own commits with `cherry-pick -x --signoff`, patch-id dedupe
against upstream. Never merge PR tips blindly.

## Deferred conflicts — the real finding

**These are duplicate implementations, not rebase noise.** #6013 applied clean and
claimed the canonical identity helper; #6235 and #6077 then collided on the same files
because all three solve overlapping problems different ways.

| PR | Conflicting files | Resolution |
|---|---|---|
| #6235 (1) | `personaCatalogRelay.ts` | Fold pubkey normalization into #6013's helper |
| #6077 (3) | `agentIdentity.ts`, `unifiedAgentGroups.ts` | Reconcile guard w/ #6013 canonical model |
| #6126 (1) | `ChannelPane.tsx` | Re-apply mention receipts over new pane |
| #6060 (3) | `workflow_sink.rs`, `executor.rs`, `schema.rs` | Rebase assign_agent onto current sink |
| #6425 (1) | `thread_detail_page.dart` | Re-apply quick actions over current page |

Resolving these **collapses 5 overlapping identity PRs into 1 canonical path** — which
is the consolidation that makes the work reviewable.

## Verification gates

Per layer: typecheck + targeted unit tests. Full stack: `just ci`, then the E2E scenario.

Contract tests still owed: identity normalization, duplicate delivery, relay
disconnect/reconnect, assignment authorization, acknowledgements, task event ordering.

## Target E2E demo

1. Bot message enters via Hermes → 2. Buzz creates/associates a task →
3. Buzz assigns a Hermes-backed agent → 4. Agent acks + emits progress →
5. Handoff to a second agent → 6. **Relay disconnect/reconnect does not duplicate
execution** → 7. Response returns through the originating bot → 8. Whole chain visible
in Buzz's audit trail.

## Boundaries (non-negotiable)

- No Telegram/Discord secrets in Buzz.
- Never infer identity from mutable display names.
- Inbound message must not trigger unrestricted tool execution.
- Bridge-generated messages must not recursively re-enter the bridge.
- Call it **agent coordination**, not "agent economy" — economic primitives need
  authorization, metering, reputation, settlement, and disputes first. The weaker
  claim is the more credible one.

## Upstream policy going forward

Stop opening new small defect fixes — we lose those races. Keep upstream diffs narrow,
pick only the 2-3 with strategic value, and consolidate on the fork instead.
