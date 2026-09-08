# Fleet qualification adapter

This first integration accepts a signed CML plan, dispatches a fixed repository
qualification operation to its assigned PC, and submits evidence to CML review.
It does not run an AI prompt, edit a repository, approve a review, merge, or
deploy. A successful qualification is not evidence that model fallback works.

The executor reads the configured repository's Git HEAD and indexed filename
count. It does not determine whether the worktree is clean. It records its
Python version and a receipt hash. The host stores the
complete result; CML evidence carries its hash. Artifact upload and a phone
result viewer are not implemented by this adapter.

## Verify locally

From the repository root:

```sh
python3 scripts/fleet/fleet.py --help
python3 -W error::ResourceWarning -m unittest discover -s scripts/fleet -p 'test_*.py' -v
```

The suite runs the actual adapter CLI, a subprocess host, and a temporary Git
repository. Its fake Buzz CLI injects transport failure; it does **not** prove
Nostr signature validation. A live run with the product Buzz CLI and signed CML
events remains a separate rollout gate. The tests also exercise lost replies,
duplicate delivery, scope rejection, unknown-attempt recovery refusal, and
cancellation before and during a probe.

## Operator policy

Policy is a host-local JSON file, selected with the required `--policy` flag.
Keep API keys out of this file. `BUZZ_PRIVATE_KEY` and optional `BUZZ_AUTH_TAG` are
resolved by the native Buzz CLI on the host where it runs.

Required common keys are `version` (1), `alias`, `relay`, `buzz_binary`,
`buzz_binary_sha256`, `state_dir`, `receipts_path`, `planner_pubkeys`, and
`channels`. Paths are absolute. The SHA-256 pins the installed native Buzz
binary; stock builds lacking `cml events` cannot be used. The planner and
channel allowlists must be nonempty.

An execution host additionally defines `worker_pubkey`, `git_binary`, and
`repositories`: each repository alias maps to an object with its canonical
`id` (for example, `mfethe1/buzz`) and its local worktree-root `path`.
The scheduler defines `hosts`, keyed by host alias. Each host entry contains
the same `worker_pubkey` and repository IDs, plus a fixed `command` argv that
starts the host adapter's `execute` command over authenticated SSH or locally.
Repository paths and commands never come from task content. Do not put private
keys or bearer credentials into the command argv.

Use `receipts_backend: "hermes"` only when the selected Python environment can
import Mack's installed `hermes_bridge.receipts.ReceiptStore`; set
`receipts_path` to the existing bridge receipt database. Otherwise the default
`compatible` backend creates the same table/API shape in the configured file.
This does not enqueue work into the existing unrestricted Codex bridge runner.

A CML plan's `extensions.org.buzz.fleet.v1` object must have exactly four keys:
`target` (host alias), `repository` (policy alias), `capability` (`qualify`), and
`expires_at` (Unix seconds, after the plan timestamp and at most one hour later).
The plan must assign the configured worker and an allowlisted planner. It must
already be signed and accepted by Buzz. Human/mobile task rows are not implicitly
converted into executable plans.

## Runtime contract

`admit --channel UUID --task UUID` fetches and reduces native signed CML, then
records an execution request. It returns a stable `attempt_id` bound to relay,
channel, task UUID, and plan event ID. `dispatch --attempt ID` delivers it once.
`result --attempt ID` reads the local durable receipt/journal. After a lost
transport reply, `recover --attempt ID` sends the same frozen request; a host
with a started receipt reports `outcome_unknown` and refuses to run it again.

Each host independently reduces the current CML chain, checks scope and the
shared admission-policy digest, and fails closed if the head changed or the
chain conflicts. A kernel lock serializes its driver. Frozen claim/start/result
snapshots are persisted before publication, preserving the Nostr event ID on
retry. An execution receipt is committed before the fixed probe starts, and
the result is committed before its CML submission. Successful execution reaches
`review`; independent verification is still required.

The current relay rejects signed event timestamps outside a 15-minute window.
An outbox can reconcile an already accepted event after that window, but an
unaccepted frozen event can then require explicit reconciliation. This adapter
does not silently create a competing successor with a new timestamp.

`cancel --attempt ID` requires an already signed planner cancellation in CML.
It forwards the request to the host, where `cancel_requested` remains intent.
`cancelled_before_execution` or a terminal `cancel_acknowledged` receipt is the
acknowledgement. A transport failure never proves that a remote process stopped.

This is an on-demand driver, not an always-running subscriber or a complete
fleet scheduler. Qualification dispatch is serialized. Shared provider quotas,
agent sandboxing, phone approvals/push, automatic admission, and AI execution
remain outside this slice. Windows execution admission is explicitly blocked
until process-tree containment has been implemented and qualified on that host.
POSIX process groups bound the known fixed commands; they do not contain a
deliberately detached descendant and are not a sandbox for untrusted agents.
