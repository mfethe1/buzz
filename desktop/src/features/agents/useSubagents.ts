/**
 * React bindings for live subagent records (SPEC-nested-subagents).
 *
 * Source of truth: `subagentsByParent` in observerRelayStore, fed by
 * `subagent_lifecycle` observer events the ACP harness publishes under the
 * parent agent's tag. This hook only reads; the store owns ingestion,
 * dedupe, and reset semantics.
 */

import * as React from "react";

import {
  getAllSubagents,
  subscribeSubagentLifecycle,
} from "./observerRelayStore";
import type { SubagentStatus } from "./lib/subagents";

/**
 * All live subagent records across parents. Re-renders only when a lifecycle
 * event actually changed a record (ingestion returns false for duplicates).
 */
export function useSubagents(): readonly SubagentStatus[] {
  return React.useSyncExternalStore(
    subscribeSubagentLifecycle,
    getAllSubagents,
    getAllSubagents,
  );
}
