/**
 * Pure subagent-lifecycle ingestion (SPEC-nested-subagents).
 *
 * Kept free of `@/` alias imports so it is testable under plain `node --test`
 * (same constraint as `lib/subagents.ts`). The observer relay store owns the
 * live map; this module holds the parse/dedupe/update logic.
 *
 * Wire shape (crates/buzz-acp/src/subagent.rs): the harness publishes
 * lifecycle events under its own (parent) agent tag, so the record's parent
 * pubkey comes from the frame, not the payload. Payload:
 * `{subagent_name, status ∈ spawned|running|complete|failed, summary?}`.
 */

import type { SubagentStatus } from "./subagents.ts";

export type SubagentLifecyclePayload = {
  subagent_name: string;
  status: SubagentStatus["status"];
  summary?: string;
};

/**
 * Parse one `subagent_lifecycle` payload. Returns null for anything
 * malformed — a bad frame must never blank or pollute the tree.
 */
export function parseSubagentLifecyclePayload(
  payload: unknown,
): SubagentLifecyclePayload | null {
  if (typeof payload !== "object" || payload === null) {
    return null;
  }
  const record = payload as {
    subagent_name?: unknown;
    status?: unknown;
    summary?: unknown;
  };
  if (typeof record.subagent_name !== "string" || record.subagent_name === "") {
    return null;
  }
  if (
    record.status !== "spawned" &&
    record.status !== "running" &&
    record.status !== "complete" &&
    record.status !== "failed"
  ) {
    return null;
  }
  const summary =
    typeof record.summary === "string" && record.summary.length > 0
      ? record.summary
      : undefined;
  return {
    subagent_name: record.subagent_name,
    status: record.status,
    summary,
  };
}

/**
 * Fold one parsed lifecycle payload into a parent's record list. Returns the
 * next list when the record changed (new subagent, status transition, or
 * summary update), or null when it is a no-op duplicate so the caller skips
 * notifying listeners. A same-name respawn replaces the prior row rather
 * than stacking.
 */
export function foldSubagentLifecycle(
  list: readonly SubagentStatus[],
  parentPubkey: string,
  parsed: SubagentLifecyclePayload,
  lastActiveAt: number,
): SubagentStatus[] | null {
  const existingIndex = list.findIndex(
    (entry) => entry.name === parsed.subagent_name,
  );
  if (existingIndex >= 0) {
    const existing = list[existingIndex];
    if (
      existing.status === parsed.status &&
      existing.summary === parsed.summary &&
      !Number.isNaN(lastActiveAt) &&
      existing.lastActiveAt >= lastActiveAt
    ) {
      return null;
    }
  }
  const next: SubagentStatus = {
    id: `${parentPubkey}:${parsed.subagent_name}`,
    name: parsed.subagent_name,
    parentPubkey,
    status: parsed.status,
    lastActiveAt: Number.isNaN(lastActiveAt) ? Date.now() : lastActiveAt,
    summary: parsed.summary,
  };
  return existingIndex >= 0
    ? list.slice(0, existingIndex).concat([next], list.slice(existingIndex + 1))
    : [...list, next];
}
