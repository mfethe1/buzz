import { makeNip98AuthHeader } from "@/shared/lib/nip98";
import { relayHttpBaseUrl } from "@/shared/lib/relay-url";
import { signNostrEvent } from "@/shared/lib/nostr-signer";
import { parse } from "yaml";

export type Workflow = {
  id: string;
  channelId: string;
  owner: string;
  revision: string;
  yaml: string;
};
export type Run = {
  id: string;
  status: string;
  current_step: number;
  error_message: string | null;
};
type Event = { id: string; pubkey: string; content: string; tags: string[][] };
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export function validId(value: string): boolean {
  return uuid.test(value);
}
export function workflowSummary(yaml: string): {
  name: string;
  manual: boolean;
  webhook: boolean;
  enabled: boolean;
  valid: boolean;
} {
  try {
    const value: unknown = parse(yaml, { uniqueKeys: true });
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw new Error("Invalid definition.");
    const definition = value as Record<string, unknown>;
    const trigger =
      definition.trigger &&
      typeof definition.trigger === "object" &&
      !Array.isArray(definition.trigger)
        ? (definition.trigger as Record<string, unknown>)
        : {};
    return {
      name:
        typeof definition.name === "string"
          ? definition.name
          : "Unnamed workflow",
      manual: trigger.on === "manual",
      webhook: trigger.on === "webhook",
      enabled: definition.enabled !== false,
      valid: true,
    };
  } catch {
    return {
      name: "Invalid definition",
      manual: false,
      webhook: false,
      enabled: false,
      valid: false,
    };
  }
}

async function request(
  path: string,
  method: "GET" | "POST",
  data?: unknown,
): Promise<unknown> {
  const url = `${relayHttpBaseUrl()}${path}`;
  const body = data === undefined ? undefined : JSON.stringify(data);
  const authorization = await makeNip98AuthHeader(url, method, {
    body,
    requireNip07: true,
  });
  const response = await fetch(url, {
    method,
    headers: {
      Authorization: authorization,
      ...(body ? { "Content-Type": "application/json" } : {}),
    },
    body,
  });
  const result: unknown = await response.json();
  if (!response.ok) {
    const detail =
      typeof result === "object" && result !== null && "error" in result
        ? String(result.error)
        : `HTTP ${response.status}`;
    throw new Error(detail);
  }
  return result;
}

function tag(event: Event, key: string): string | undefined {
  return event.tags.find((entry) => entry[0] === key)?.[1];
}

export async function listWorkflows(channelId: string): Promise<Workflow[]> {
  if (!validId(channelId)) throw new Error("Enter a valid channel UUID.");
  const result = await request("/query", "POST", [
    { kinds: [30620], "#h": [channelId] },
  ]);
  if (!Array.isArray(result))
    throw new Error("Invalid relay workflow response.");
  return (result as Event[])
    .map((event) => ({
      id: tag(event, "d") ?? "",
      channelId: tag(event, "h") ?? "",
      owner: event.pubkey,
      revision: event.id,
      yaml: event.content,
    }))
    .filter(
      (workflow) => validId(workflow.id) && workflow.channelId === channelId,
    );
}

async function submit(
  kind: number,
  tags: string[][],
  content: string,
): Promise<string> {
  const event = await signNostrEvent(
    { kind, tags, content },
    { requireNip07: true },
  );
  const result = await request("/events", "POST", event);
  if (
    !result ||
    typeof result !== "object" ||
    !("accepted" in result) ||
    result.accepted !== true
  ) {
    throw new Error(
      result && typeof result === "object" && "message" in result
        ? String(result.message)
        : "Relay did not accept the event.",
    );
  }
  return event.id;
}

export async function createWorkflow(
  channelId: string,
  yaml: string,
): Promise<string> {
  if (!validId(channelId)) throw new Error("Enter a valid channel UUID.");
  if (!yaml.trim()) throw new Error("Enter a workflow definition.");
  // The relay returns webhook credentials only once; never silently discard them.
  const summary = workflowSummary(yaml);
  if (!summary.valid) throw new Error("Invalid workflow YAML.");
  if (summary.webhook)
    throw new Error(
      "Webhook creation is not supported in web yet. Use desktop to retain its secret.",
    );
  const id = crypto.randomUUID();
  await submit(
    30620,
    [
      ["d", id],
      ["h", channelId],
    ],
    yaml,
  );
  return id;
}

export async function triggerWorkflow(
  workflow: Workflow,
  pubkey: string,
): Promise<void> {
  if (!validId(workflow.id) || workflow.owner !== pubkey)
    throw new Error("Only the workflow owner can run it.");
  const summary = workflowSummary(workflow.yaml);
  if (!summary.valid || !summary.enabled || !summary.manual)
    throw new Error("Only enabled manual workflows can be run from web.");
  await submit(46020, [["d", workflow.id]], "");
}

export async function listRuns(workflowId: string): Promise<Run[]> {
  if (!validId(workflowId)) throw new Error("Invalid workflow id.");
  const result = await request(`/workflows/${workflowId}/runs?limit=20`, "GET");
  if (
    !result ||
    typeof result !== "object" ||
    !("runs" in result) ||
    !Array.isArray(result.runs)
  ) {
    throw new Error("Invalid relay run response.");
  }
  return result.runs as Run[];
}
