import * as React from "react";
import type { AgentAvailabilityReader } from "@/features/agents/lib/useAgentAvailability";
import type {
  ManagedAgent,
  PresenceStatus,
  RelayAgent,
} from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

/**
 * Relay-agent liveness for the mention path.
 *
 * The kind:10100 agent directory carries a replaceable, unexpiring `status`, so
 * a crashed agent keeps advertising "online" forever. TTL'd relay presence is
 * the authority when it is knowable. `undefined` availability means the read
 * could not establish anything (presence not loaded, relay disconnected, or the
 * pubkey was outside the queried set) — fall back to the directory rather than
 * inferring inactive, or a relay blip empties the mention picker. Presence can
 * therefore only ever demote a directory-online agent on a *successful* offline
 * read.
 */
export function isRelayAgentActive(
  directoryStatus: string,
  availability: PresenceStatus | undefined,
): boolean {
  return availability === undefined
    ? directoryStatus === "online" || directoryStatus === "away"
    : availability === "online" || availability === "away";
}

export function useActiveAgentPubkeys(
  managedAgents?: readonly ManagedAgent[],
  relayAgents?: readonly RelayAgent[],
  getAvailability?: AgentAvailabilityReader,
): ReadonlySet<string> {
  return React.useMemo(
    () =>
      new Set([
        ...(managedAgents ?? [])
          .filter(
            (agent) =>
              agent.status === "running" || agent.status === "deployed",
          )
          .map((agent) => normalizePubkey(agent.pubkey)),
        ...(relayAgents ?? [])
          .filter((agent) =>
            isRelayAgentActive(
              agent.status,
              getAvailability?.(agent.pubkey) ?? undefined,
            ),
          )
          .map((agent) => normalizePubkey(agent.pubkey)),
      ]),
    [managedAgents, relayAgents, getAvailability],
  );
}
