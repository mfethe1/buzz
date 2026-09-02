/**
 * Subagent nesting types + pure selectors (Workstream B, SPEC-nested-subagents).
 *
 * Workstream A (buzz-acp adapter) will stream lifecycle events carrying tag
 * `["parent", "<parent-pubkey-hex>"]` with payload
 * `{subagent_name, parent_pubkey, status ∈ spawned|running|complete|failed,
 * summary?}`. Until those events land, the desktop UI is wired against the
 * `SubagentStatus` type and the pure selectors here, so the tree renders from
 * whatever list a data source provides — live events, a fixture, or empty.
 *
 * Nothing in this module decides which agents exist. Parents come from the
 * existing managed-agent list; a subagent whose parent is not (yet) loaded is
 * surfaced in `orphans` so no child is silently dropped — the caller decides
 * how to present an unresolvable parent.
 */

export type SubagentLifecycleStatus =
  | "spawned"
  | "running"
  | "complete"
  | "failed";

export type SubagentStatus = {
  /** Stable row key; derived from parent pubkey + id by the data source. */
  id: string;
  name: string;
  parentPubkey: string;
  status: SubagentLifecycleStatus;
  /** Epoch ms of the last lifecycle update; drives the idle-time label. */
  lastActiveAt: number;
  summary?: string;
};

/** Lifecycle statuses that mean the subagent is doing something right now. */
export const ACTIVE_SUBAGENT_STATUSES: ReadonlySet<SubagentLifecycleStatus> =
  new Set(["spawned", "running"]);

export function isActiveSubagent(subagent: SubagentStatus): boolean {
  return ACTIVE_SUBAGENT_STATUSES.has(subagent.status);
}

export type SubagentGroup = {
  /** Parent pubkey, normalized (trimmed, lowercased) — the map key. */
  parentPubkey: string;
  subagents: SubagentStatus[];
  activeCount: number;
};

export type SubagentGrouping = {
  /** Parent-pubkey → children, first-seen order. */
  byParent: Map<string, SubagentGroup>;
  /**
   * Children whose parent pubkey is not in `parentPubkeys`. Kept (not
   * dropped) so the render layer can decide how to surface a parent that is
   * unloaded, archived, or from another device.
   */
  orphans: SubagentStatus[];
};

/**
 * Group subagent records under their parent pubkeys. Pure: no clock reads,
 * no filtering decisions smuggled in. Parent pubkeys are normalized with the
 * canonical `normalizePubkey`; `parentPubkey` on each record is normalized
 * the same way so consumers can index by either without re-normalizing.
 */
export function groupSubagentsByParent(
  subagents: readonly SubagentStatus[],
  parentPubkeys: readonly string[],
): SubagentGrouping {
  const knownParents = new Set(parentPubkeys.map(normalize));

  const byParent = new Map<string, SubagentGroup>();
  const orphans: SubagentStatus[] = [];

  for (const record of subagents) {
    const normalized = normalize(record.parentPubkey);
    const subagent: SubagentStatus = { ...record, parentPubkey: normalized };

    if (!knownParents.has(normalized)) {
      orphans.push(subagent);
      continue;
    }

    const group = byParent.get(normalized);
    if (group) {
      group.subagents.push(subagent);
      if (isActiveSubagent(subagent)) group.activeCount += 1;
    } else {
      byParent.set(normalized, {
        parentPubkey: normalized,
        subagents: [subagent],
        activeCount: isActiveSubagent(subagent) ? 1 : 0,
      });
    }
  }

  return { byParent, orphans };
}

/**
 * The "(N active)" count for a parent row. Active = spawned or running;
 * completed and failed children stay listed under the parent once expanded
 * but do not count toward the badge, matching the SPEC's "live count".
 */
export function activeSubagentCount(
  subagents: readonly SubagentStatus[],
): number {
  return subagents.reduce(
    (count, subagent) => count + (isActiveSubagent(subagent) ? 1 : 0),
    0,
  );
}

/** Idle-time label for a subagent row: "5m 12s" since its last update. */
export function subagentIdleLabel(
  subagent: SubagentStatus,
  nowMs: number,
): string {
  return formatIdleDuration(Math.max(0, nowMs - subagent.lastActiveAt));
}

function formatIdleDuration(ms: number): string {
  const totalSeconds = Math.floor(ms / 1000);
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const seconds = totalSeconds % 60;
  const totalMinutes = Math.floor(totalSeconds / 60);
  if (totalMinutes < 60) return `${totalMinutes}m ${seconds}s`;
  const minutes = totalMinutes % 60;
  const hours = Math.floor(totalMinutes / 60);
  return `${hours}h ${minutes}m ${seconds}s`;
}

function normalize(pubkey: string): string {
  return pubkey.trim().toLowerCase();
}
