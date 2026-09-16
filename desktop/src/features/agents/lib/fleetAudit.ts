import type { RelayAgent } from "@/shared/api/relayDirectoryTypes";
import type { ManagedAgent, ManagedAgentBackend } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

/** "local" or the provider id — the run-location column value (#55). */
function describeBackend(backend: ManagedAgentBackend): string {
  return backend.type === "local" ? "local" : backend.id;
}

/**
 * One row of the fleet audit table (#55): everything known about an agent
 * across origin device, owner, run location, and local activity — joined from
 * relay directory data (owner-attested) and this machine's managed-agent
 * state (local ground truth). No new wire protocol; pure aggregation.
 */
export type FleetAuditRow = {
  pubkey: string;
  name: string;
  /** Origin: human device label from kind:30177, or null pre-feature. */
  deviceLabel: string | null;
  /** Origin: opaque device id from kind:30177 (stable per install). */
  deviceId: string | null;
  /** Origin: earliest verified 30177 sighting, unix seconds. */
  firstSeen: number | null;
  /** Owner pubkey (NIP-OA verified), or null on legacy entries. */
  ownerPubkey: string | null;
  /** True when the owner is the viewer on this machine. */
  ownedByViewer: boolean;
  /** Discovery liveness: "online" | "away" | "offline" | "unknown". */
  status: RelayAgent["status"];
  /** Run location: "local" when this machine holds the agent's secret. */
  runsHere: boolean;
  /** Local run status, or null when the agent is not on this machine. */
  localStatus: ManagedAgent["status"] | null;
  /** Local last-active (last start) timestamp, when it runs here. */
  lastStartedAt: string | null;
  /** Local backend ("local" or provider id), when it runs here. */
  backend: string;
  /** Relay channels this agent is a member of (ids). */
  channelIds: string[];
};

/**
 * Join the relay directory with local managed-agent state into fleet-audit
 * rows. Union on pubkey: relay-only agents appear with `runsHere: false`,
 * local-only agents appear with origin fields from the directory when
 * present (else null).
 */
export function buildFleetAuditRows(
  relayAgents: RelayAgent[],
  managedAgents: Pick<
    ManagedAgent,
    "pubkey" | "name" | "status" | "lastStartedAt" | "backend"
  >[],
  viewerPubkey: string | null,
): FleetAuditRow[] {
  const managedByPubkey = new Map(
    managedAgents.map((agent) => [agent.pubkey, agent]),
  );
  const rows = new Map<string, FleetAuditRow>();

  for (const agent of relayAgents) {
    const local = managedByPubkey.get(agent.pubkey) ?? null;
    rows.set(agent.pubkey, {
      pubkey: agent.pubkey,
      name: agent.name,
      deviceLabel: agent.deviceLabel,
      deviceId: agent.deviceId,
      firstSeen: agent.firstSeen,
      ownerPubkey: agent.ownerPubkey,
      ownedByViewer:
        viewerPubkey !== null &&
        normalizePubkey(agent.ownerPubkey ?? "") ===
          normalizePubkey(viewerPubkey),
      status: agent.status,
      runsHere: local !== null,
      localStatus: local?.status ?? null,
      lastStartedAt: local?.lastStartedAt ?? null,
      backend: local ? describeBackend(local.backend) : "—",
      channelIds: agent.channelIds,
    });
  }

  // Local-only agents (never published / directory not loaded yet).
  for (const agent of managedAgents) {
    if (rows.has(agent.pubkey)) continue;
    rows.set(agent.pubkey, {
      pubkey: agent.pubkey,
      name: agent.name,
      deviceLabel: null,
      deviceId: null,
      firstSeen: null,
      ownerPubkey: viewerPubkey,
      ownedByViewer: viewerPubkey !== null,
      status: "unknown",
      runsHere: true,
      localStatus: agent.status,
      lastStartedAt: agent.lastStartedAt,
      backend: describeBackend(agent.backend),
      channelIds: [],
    });
  }

  return [...rows.values()].sort((left, right) => {
    // Local agents first, then by name.
    if (left.runsHere !== right.runsHere) return left.runsHere ? -1 : 1;
    return left.name.localeCompare(right.name);
  });
}

/** Render `firstSeen` unix seconds as a short local date, or a dash. */
export function formatFirstSeen(firstSeen: number | null): string {
  if (firstSeen === null || !Number.isFinite(firstSeen)) return "—";
  return new Date(firstSeen * 1000).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

/** Short `pubkey…` form for owner/device id columns. */
export function shortId(value: string | null): string {
  if (!value) return "—";
  return `${value.slice(0, 8)}…`;
}
