# Fleet qualification adapter

This adapter accepts a signed CML plan for an existing Buzz task, dispatches a
fixed repository qualification to its assigned worker, and records a signed
outcome against the task and attempt. A successful probe submits CML evidence
for independent review. It does not run an AI prompt, edit a repository, approve
work, merge, or deploy. Qualification does not exercise model fallbacks.

The probe reads the configured repository's Git HEAD and indexed filename
count. It does not determine whether the worktree is clean. It records the
Python version in the host's durable receipt. A typed signed receipt containing
that bounded result is persisted by the relay; CML evidence references its event
ID. Artifact upload and a phone result viewer are outside this adapter.

## Verify locally

From the repository root, with Python 3 and Git available:

```sh
python3 scripts/fleet/fleet.py --help
python3 -W error::ResourceWarning -m unittest discover -s scripts/fleet -p 'test_*.py' -v
```

These tests run the actual adapter CLI, a subprocess host, and a temporary Git
repository. Their fake Buzz CLI injects transport failures; it does **not** prove
Nostr verification or relay authorization. Signed native relay qualification is
a separate gate. PostgreSQL tests exercise atomic admission, revocation ordering,
HTTP authentication, receipt projection, and whole-community cleanup through
the repository's isolated PostgreSQL lane.

## Operator policy and signed scope

Policy is host-local JSON, selected by the required `--policy` flag. Keep API
keys out of it. The native Buzz CLI resolves `BUZZ_PRIVATE_KEY` and optional
`BUZZ_AUTH_TAG` from its host environment.

Common configuration includes `version` (2), `alias`, `relay`, `buzz_binary`,
`buzz_binary_sha256`, `state_dir`, `receipts_path`, `planner_pubkeys`, and
`channels`. Paths are absolute. The SHA-256 pins the installed native Buzz
binary. Planner and channel allowlists must be nonempty. The relay must have the
atomic fleet admission schema and handlers, including migration 0052; an
ordinary event-only relay cannot authorize execution.

An execution host also defines `worker_pubkey`, stable registered `machine_id`,
`git_binary`, and `repositories`. Each repository alias maps to a canonical `id`
(for example, `mfethe1/buzz`) and a local worktree-root `path`. The scheduler
provides `hosts`, keyed by host alias. Each entry contains that worker,
`machine_id`, repository IDs, and a fixed `command` argv that starts the host's
`execute` command locally or over authenticated SSH. Repository paths and argv
never come from task content. Do not put credentials in argv.

The `policy-digest --target HOST` command computes the public scope digest from
this configuration. Scheduler and host must agree on version, target, machine,
relay, worker, planner/channel allowlists, and repository IDs. The digest does
not embed host-local paths or credentials; the binary pin is checked separately.

Use `receipts_backend: "hermes"` only when that Python environment can import
Mack's installed `hermes_bridge.receipts.ReceiptStore`, with `receipts_path`
pointing to its existing receipt database. The default `compatible` backend uses
the same schema/API shape in its configured file. This does not enqueue work into
the unrestricted Codex bridge runner.

A CML plan's `extensions.org.buzz.fleet.v2` object has exactly seven keys:
`target`, `machine_id`, `repository`, `capability` (`qualify`), `expires_at`,
`task_revision`, and `policy_digest`. The expiry is after the plan timestamp and
at most one hour later. The plan UUID must identify an existing, unarchived task
in the same channel, at that exact revision. Both planner and worker must be
active explicit channel members. The worker must match its registered machine
home; the planner must have an active existing `cross_ssh` capability grant for
that machine or `*`. This adapter does not enroll identities or grant permission.
Task assignee and status do not confer execution permission.

There is one qualification attempt per CML task root. A new qualification needs
an explicitly created new task and signed plan; an old root is never silently
replaced. Version 1 fleet requests are rejected by this adapter.

## Runtime and recovery

`admit --channel UUID --task UUID` validates native CML and its relay attempt
projection, then records the frozen request. The relay's stable attempt ID binds
community, task UUID, and signed plan event ID. `dispatch --attempt ID` delivers
it once. `result --attempt ID` reads local receipts; `recover --attempt ID`
redelivers the same request to reconcile a previous uncertain delivery.

The relay commits each fleet event and its attempt projection atomically after
ordinary signed-event ingress checks. Plan, claim, and start check current
permissions; positive grant, home, membership and lifecycle locks order admission
with revocation. Before its local receipt claim and probe, the worker requires
both a newly accepted response to its own signed start and a fresh primary
admission read bound to that start, task revision, attempt, worker and scope.
Missing projections, denied reads, duplicate ACKs and lost start replies fail
closed. A reduced CML snapshot or task display response cannot authorize a probe.

A host commits its start intent before network publication and its local receipt
before the probe. An existing start intent or unfinished receipt is never replayed;
it reports `outcome_unknown`. A started relay projection proves acceptance of a
start, not that a process is running or finished. A known worker may append a
signed `unknown` outcome; that still does not acknowledge stopping.

Results are saved locally before delivery. Typed receipts use protocol
`buzz-fleet-receipt` version 1 and the attempt ID as their `d` tag, keeping them
outside the CML task's reducer. A delayed receipt can refresh its publication
envelope while retaining its frozen result and completion time. CML lifecycle
outboxes retain their original snapshots: an already accepted event can be
reconciled, but an unaccepted event outside ingest's 15-minute timestamp window
requires explicit reconciliation. Such a failure cannot restart the probe.

`cancel --attempt ID` requires an already signed planner cancellation. The relay
records intent first. A host reports `cancel_requested` while a known probe is
being stopped, and publishes a terminal receipt only after its process runner
returns. Cancellation before any accepted start can produce
`cancelled_before_execution`. A terminal `cancel_acknowledged` may also mean a
probe had already finished; inspect the typed outcome. A crashed or unreachable
worker cannot turn uncertainty into proof of stopping.

This is an on-demand fixed-operation driver. An always-running Mack scheduler,
shared provider quotas, agent sandboxing, phone approvals/push, and AI execution
remain separate work. Windows execution is blocked until process-tree
containment is qualified there. POSIX process groups bound the known fixed
commands during normal cancellation; they do not contain deliberately detached
descendants or prove that a child stopped after its adapter crashed.
