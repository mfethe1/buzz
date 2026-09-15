import * as React from "react";
import { MonitorSmartphone, RefreshCw } from "lucide-react";

import {
  buildFleetAuditRows,
  formatFirstSeen,
  shortId,
} from "@/features/agents/lib/fleetAudit";
import type { RelayAgent } from "@/shared/api/relayDirectoryTypes";
import type { ManagedAgent } from "@/shared/api/types";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { SectionHeader } from "@/shared/ui/PageHeader";

type FleetAuditSectionProps = {
  relayAgents: RelayAgent[];
  relayAgentsError: Error | null;
  isRelayLoading: boolean;
  managedAgents: Pick<
    ManagedAgent,
    "pubkey" | "name" | "status" | "lastStartedAt" | "backend"
  >[];
  viewerPubkey: string | null;
  onRefresh: () => void;
  isRefreshing: boolean;
};

const STATUS_TONE: Record<RelayAgent["status"], string> = {
  online: "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400",
  away: "bg-amber-500/15 text-amber-600 dark:text-amber-400",
  offline: "bg-muted text-muted-foreground",
  unknown: "bg-muted text-muted-foreground",
};

export function FleetAuditSection({
  relayAgents,
  relayAgentsError,
  isRelayLoading,
  managedAgents,
  viewerPubkey,
  onRefresh,
  isRefreshing,
}: FleetAuditSectionProps) {
  const rows = React.useMemo(
    () => buildFleetAuditRows(relayAgents, managedAgents, viewerPubkey),
    [relayAgents, managedAgents, viewerPubkey],
  );
  const localCount = rows.filter((row) => row.runsHere).length;
  const deviceCount = new Set(
    rows.filter((row) => row.deviceId).map((row) => row.deviceId),
  ).size;

  return (
    <section className="relative space-y-4" data-testid="agents-fleet-audit">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <SectionHeader
          title="Fleet audit"
          description="Every agent this community knows: origin device, owner, run location, and activity."
        />
        <Button
          variant="outline"
          size="sm"
          onClick={onRefresh}
          disabled={isRefreshing}
          data-testid="agents-fleet-audit-refresh"
        >
          <RefreshCw className={isRefreshing ? "animate-spin" : undefined} />
          Refresh
        </Button>
      </div>

      <p
        className="text-xs text-muted-foreground"
        data-testid="agents-fleet-audit-summary"
      >
        {rows.length} agent{rows.length === 1 ? "" : "s"} · {localCount} on this
        machine · {deviceCount} origin device{deviceCount === 1 ? "" : "s"}
      </p>

      {relayAgentsError ? (
        <p
          className="text-sm text-destructive"
          data-testid="agents-fleet-audit-error"
        >
          Relay directory unavailable: {relayAgentsError.message}. Local agents
          are still listed below.
        </p>
      ) : null}

      {isRelayLoading && rows.length === 0 ? (
        <p
          className="text-sm text-muted-foreground"
          data-testid="agents-fleet-audit-loading"
        >
          Loading fleet…
        </p>
      ) : rows.length === 0 ? (
        <p
          className="text-sm text-muted-foreground"
          data-testid="agents-fleet-audit-empty"
        >
          No agents yet.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table
            className="w-full border-collapse text-sm"
            data-testid="agents-fleet-audit-table"
          >
            <thead>
              <tr className="border-b text-left text-xs text-muted-foreground">
                <th className="px-3 py-2 font-normal">Agent</th>
                <th className="px-3 py-2 font-normal">Origin device</th>
                <th className="px-3 py-2 font-normal">Owner</th>
                <th className="px-3 py-2 font-normal">Runs</th>
                <th className="px-3 py-2 font-normal">Liveness</th>
                <th className="px-3 py-2 font-normal">First seen</th>
                <th className="px-3 py-2 font-normal">Last active</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr
                  key={row.pubkey}
                  className="border-b last:border-b-0 hover:bg-muted/40"
                  data-testid="agents-fleet-audit-row"
                  data-agent-pubkey={row.pubkey}
                >
                  <td className="max-w-56 truncate px-3 py-2 font-medium">
                    {row.name}
                  </td>
                  <td className="px-3 py-2">
                    {row.deviceLabel ? (
                      <span className="inline-flex items-center gap-1.5">
                        <MonitorSmartphone className="size-3.5 text-muted-foreground" />
                        {row.deviceLabel}
                      </span>
                    ) : (
                      <span
                        className="text-muted-foreground"
                        title={row.deviceId ?? undefined}
                      >
                        {shortId(row.deviceId)}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2 text-muted-foreground">
                    {row.ownedByViewer ? (
                      <span title={row.ownerPubkey ?? undefined}>You</span>
                    ) : (
                      <span title={row.ownerPubkey ?? undefined}>
                        {shortId(row.ownerPubkey)}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2">
                    {row.runsHere ? (
                      <Badge variant="secondary">{row.backend}</Badge>
                    ) : (
                      <span className="text-muted-foreground">
                        other device
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2">
                    <span
                      className={`inline-flex items-center rounded-full px-2 py-0.5 text-xs ${STATUS_TONE[row.status]}`}
                    >
                      {row.status}
                    </span>
                  </td>
                  <td className="px-3 py-2 text-muted-foreground">
                    {formatFirstSeen(row.firstSeen)}
                  </td>
                  <td className="px-3 py-2 text-muted-foreground">
                    {row.lastStartedAt
                      ? new Date(row.lastStartedAt).toLocaleDateString(
                          undefined,
                          {
                            year: "numeric",
                            month: "short",
                            day: "numeric",
                          },
                        )
                      : "—"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
