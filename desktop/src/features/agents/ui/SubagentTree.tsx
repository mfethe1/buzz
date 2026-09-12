import * as React from "react";
import { ChevronDown, ChevronRight } from "lucide-react";

import {
  activeSubagentCount,
  groupSubagentsByParent,
  type SubagentStatus,
} from "@/features/agents/lib/subagents";
import { formatElapsed } from "@/features/agents/ui/agentSessionUtils";
import { useNow } from "@/shared/lib/useNow";
import { cn } from "@/shared/lib/cn";

/**
 * Nested subagent tree for the Agents library (SPEC-nested-subagents, B2).
 *
 * Pure presentation over the `SubagentStatus` selector output: every parent
 * row is default-collapsed (the collapsed set starts with the key in it, so
 * nothing auto-expands), shows a live "(N active)" badge, and expands to one
 * line per subagent — status dot, name, idle time. Parents that currently
 * have no subagent records render nothing here; this component never decides
 * which agents exist.
 */
export function SubagentTree({
  parentPubkeys,
  subagents,
}: {
  /** Pubkeys of parent agents currently rendered in the library. */
  parentPubkeys: readonly string[];
  /** Live subagent records (from the Workstream A event stream, or empty). */
  subagents: readonly SubagentStatus[];
}) {
  // Default-collapsed: a key enters the set on first sight and leaves only by
  // explicit toggle. The chevron click flips membership, so "collapsed" is
  // opt-out rather than opt-in — new parents never spring open.
  const [expanded, setExpanded] = React.useState<ReadonlySet<string>>(
    new Set(),
  );
  const toggle = React.useCallback((key: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  const grouping = React.useMemo(
    () => groupSubagentsByParent(subagents, parentPubkeys),
    [subagents, parentPubkeys],
  );

  const rows = [...grouping.byParent.values()];
  // Orphans (children of an unloaded parent) surface through the selector's
  // `orphans` output for the caller to place deliberately; an empty parent
  // list must not render a stray container.
  if (rows.length === 0) return null;

  return (
    <div className="space-y-1" data-testid="subagent-tree">
      {rows.map((group) => {
        const isExpanded = expanded.has(group.parentPubkey);
        const activeCount = activeSubagentCount(group.subagents);
        return (
          <div key={group.parentPubkey}>
            <button
              aria-expanded={isExpanded}
              className="group flex w-full items-center gap-2 rounded-md px-1 py-1 text-left transition-colors hover:bg-muted/50"
              data-testid={`subagent-toggle-${group.parentPubkey}`}
              onClick={() => toggle(group.parentPubkey)}
              type="button"
            >
              {isExpanded ? (
                <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              ) : (
                <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
              )}
              <span className="text-xs font-medium text-muted-foreground">
                subagents
              </span>
              <span
                className="text-xs text-muted-foreground"
                data-testid={`subagent-active-count-${group.parentPubkey}`}
              >
                ({activeCount} active)
              </span>
            </button>
            {isExpanded ? (
              <ul
                className="ml-6 space-y-0.5"
                data-testid={`subagent-list-${group.parentPubkey}`}
              >
                {group.subagents.map((subagent) => (
                  <SubagentRow key={subagent.id} subagent={subagent} />
                ))}
              </ul>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}

function SubagentRow({ subagent }: { subagent: SubagentStatus }) {
  // The 1s tick lives at the leaf, matching WorkingBadge: only expanded rows
  // re-render each second, and collapsed parents tick never.
  const now = useNow(1000);

  return (
    <li
      className="flex items-center gap-2 text-xs"
      data-testid={`subagent-row-${subagent.id}`}
    >
      <SubagentStatusDot status={subagent.status} />
      <span className="truncate font-medium">{subagent.name}</span>
      <span className="shrink-0 text-muted-foreground">
        idle {formatElapsed(now - subagent.lastActiveAt)}
      </span>
      {subagent.summary ? (
        <span className="truncate text-muted-foreground">
          {subagent.summary}
        </span>
      ) : null}
    </li>
  );
}

function SubagentStatusDot({ status }: { status: SubagentStatus["status"] }) {
  // Colour-only signalling is guarded by the adjacent text; the dot's
  // data-testid carries the status so tests (and a11y audits) can read it.
  return (
    <span
      className={cn(
        "h-1.5 w-1.5 shrink-0 rounded-full",
        status === "running"
          ? "bg-emerald-500"
          : status === "spawned"
            ? "bg-sky-500"
            : status === "failed"
              ? "bg-destructive"
              : "bg-muted-foreground/50",
      )}
      data-testid={`subagent-status-${status}`}
      title={status}
    />
  );
}
