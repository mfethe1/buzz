import { createFileRoute } from "@tanstack/react-router";
import { useState } from "react";
import { hasNip07Provider } from "@/shared/lib/nostr-signer";
import {
  createWorkflow,
  listRuns,
  listWorkflows,
  triggerWorkflow,
  validId,
  workflowSummary,
  type Run,
  type Workflow,
} from "@/features/workflows/workflow-api";

export const Route = createFileRoute("/workflows")({
  component: WorkflowsPage,
});
const starter =
  "name: Phone check-in\ntrigger:\n  on: manual\nsteps:\n  - id: notify\n    action: send_message\n    text: Checked in from web\n";

function WorkflowsPage() {
  const [channel, setChannel] = useState("");
  const [pubkey, setPubkey] = useState("");
  const [workflows, setWorkflows] = useState<Workflow[]>([]);
  const [selected, setSelected] = useState<Workflow | null>(null);
  const [runs, setRuns] = useState<Run[]>([]);
  const [yaml, setYaml] = useState(starter);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  async function action(operation: () => Promise<void>) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await operation();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Request failed.");
    } finally {
      setBusy(false);
    }
  }

  async function refresh(id = channel) {
    const items = await listWorkflows(id);
    setWorkflows(items);
    if (selected)
      setSelected(items.find((item) => item.id === selected.id) ?? null);
  }

  return (
    <main className="mx-auto min-h-dvh max-w-3xl px-4 pb-16 pt-8 sm:px-8">
      <nav className="mb-8 flex items-center justify-between gap-3">
        <a className="font-semibold" href="/">
          Buzz
        </a>
        <span className="rounded-full border px-3 py-1 text-xs">
          Workflows · early access
        </span>
      </nav>
      <h1 className="text-3xl font-semibold tracking-tight">
        Channel workflows
      </h1>
      <p className="mt-2 text-sm text-muted-foreground">
        Sign with the same NIP-07 identity you use for Buzz. Your key never
        enters this page. A browser signer is required; phone pairing is not
        available yet.
      </p>
      {!pubkey ? (
        <section className="mt-8 rounded-xl border bg-card p-5">
          <h2 className="font-semibold">Connect identity</h2>
          <p className="my-3 text-sm text-muted-foreground">
            Use a NIP-07-capable browser already admitted to your community. No
            new account or anonymous key will be created.
          </p>
          <button
            type="button"
            className="rounded-lg bg-primary px-4 py-3 text-primary-foreground disabled:opacity-50"
            disabled={!hasNip07Provider() || busy}
            onClick={() =>
              action(async () => {
                const key = await window.nostr?.getPublicKey();
                if (!key) throw new Error("Signer unavailable.");
                setPubkey(key);
              })
            }
          >
            Connect signer
          </button>
          {!hasNip07Provider() && (
            <p role="status" className="mt-3 text-sm">
              No NIP-07 signer detected. Open in a compatible browser or
              continue in Buzz desktop.
            </p>
          )}
        </section>
      ) : (
        <>
          <p className="mt-5 break-all text-xs text-muted-foreground">
            Signer: {pubkey}
          </p>
          <form
            className="mt-6 flex flex-col gap-3 sm:flex-row"
            onSubmit={(event) => {
              event.preventDefault();
              void action(() => refresh());
            }}
          >
            <label className="flex-1 text-sm font-medium">
              Channel UUID (copy from desktop)
              <input
                className="mt-1 w-full rounded-lg border bg-background px-3 py-3 font-mono text-sm"
                value={channel}
                onChange={(event) => {
                  setChannel(event.target.value.trim());
                  setWorkflows([]);
                  setSelected(null);
                  setRuns([]);
                }}
                placeholder="00000000-0000-0000-0000-000000000000"
                aria-label="Channel UUID"
              />
            </label>
            <button
              type="submit"
              className="self-end rounded-lg bg-primary px-5 py-3 text-primary-foreground disabled:opacity-50"
              disabled={!validId(channel) || busy}
            >
              Load workflows
            </button>
          </form>
          {validId(channel) && (
            <>
              <section className="mt-8" aria-label="Workflows">
                <h2 className="text-xl font-semibold">Workflows</h2>
                {workflows.length === 0 && (
                  <p className="mt-3 text-sm text-muted-foreground">
                    No workflows loaded. Load this channel to see its
                    definitions.
                  </p>
                )}
                <ul className="mt-3 space-y-2">
                  {workflows.map((workflow) => (
                    <li key={workflow.id} className="rounded-xl border p-4">
                      <div className="flex items-center justify-between gap-3">
                        <div className="min-w-0">
                          <p className="truncate font-medium">
                            {workflowSummary(workflow.yaml).name}
                          </p>
                          <p className="truncate text-xs text-muted-foreground">
                            {workflow.id}
                          </p>
                        </div>
                        <button
                          type="button"
                          className="rounded-lg border px-3 py-2 text-sm"
                          onClick={() =>
                            action(async () => {
                              setSelected(workflow);
                              setRuns(await listRuns(workflow.id));
                            })
                          }
                          disabled={busy}
                        >
                          Runs
                        </button>
                      </div>
                      <details className="mt-3 text-sm">
                        <summary>Definition</summary>
                        <pre className="mt-2 overflow-x-auto whitespace-pre-wrap break-words rounded-lg bg-muted p-3 text-xs">
                          {workflow.yaml}
                        </pre>
                      </details>
                      <button
                        type="button"
                        className="mt-3 rounded-lg bg-primary px-4 py-2 text-sm text-primary-foreground disabled:opacity-50"
                        disabled={
                          busy ||
                          workflow.owner !== pubkey ||
                          !workflowSummary(workflow.yaml).manual ||
                          !workflowSummary(workflow.yaml).enabled
                        }
                        onClick={() =>
                          action(async () => {
                            await triggerWorkflow(workflow, pubkey);
                            setSelected(workflow);
                            setRuns(await listRuns(workflow.id));
                            setNotice(
                              "Trigger accepted. Refresh runs to track execution.",
                            );
                          })
                        }
                      >
                        Run manually
                      </button>
                    </li>
                  ))}
                </ul>
              </section>
              {selected && (
                <section className="mt-8" aria-label="Run status">
                  <div className="flex items-center justify-between">
                    <h2 className="text-xl font-semibold">Recent runs</h2>
                    <button
                      type="button"
                      className="rounded-lg border px-3 py-2 text-sm"
                      disabled={busy}
                      onClick={() =>
                        action(async () => setRuns(await listRuns(selected.id)))
                      }
                    >
                      Refresh
                    </button>
                  </div>
                  {runs.length === 0 ? (
                    <p className="mt-3 text-sm">No runs yet.</p>
                  ) : (
                    <ul className="mt-3 space-y-2">
                      {runs.map((run) => (
                        <li
                          className="rounded-lg border p-3 text-sm"
                          key={run.id}
                        >
                          <strong>{run.status}</strong> · Step{" "}
                          {run.current_step}
                          <p className="break-all text-xs text-muted-foreground">
                            {run.id}
                          </p>
                          {run.error_message && <p>{run.error_message}</p>}
                        </li>
                      ))}
                    </ul>
                  )}
                </section>
              )}
              <section className="mt-8" aria-label="Create workflow">
                <h2 className="text-xl font-semibold">Create workflow</h2>
                <p className="mt-2 text-sm text-muted-foreground">
                  Buzz YAML; validated and executed by the relay, not this
                  browser. Webhook creation is excluded because its one-time
                  secret needs a secure delivery flow.
                </p>
                <label className="mt-4 block text-sm font-medium">
                  Definition YAML
                  <textarea
                    className="mt-2 min-h-56 w-full rounded-lg border bg-background p-3 font-mono text-sm"
                    value={yaml}
                    onChange={(event) => setYaml(event.target.value)}
                  />
                </label>
                <button
                  type="button"
                  className="mt-3 rounded-lg bg-primary px-5 py-3 text-primary-foreground disabled:opacity-50"
                  disabled={busy}
                  onClick={() =>
                    action(async () => {
                      const id = await createWorkflow(channel, yaml);
                      await refresh();
                      setNotice(`Workflow ${id} created.`);
                    })
                  }
                >
                  Create on relay
                </button>
              </section>
            </>
          )}
        </>
      )}
      {error && (
        <p
          role="alert"
          className="mt-5 rounded-lg border border-destructive p-3 text-sm text-destructive"
        >
          {error}
        </p>
      )}
      {notice && (
        <p role="status" className="mt-5 rounded-lg border p-3 text-sm">
          {notice}
        </p>
      )}
    </main>
  );
}
