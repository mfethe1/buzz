import type { ManagedAgent, PresenceLookup } from "@/shared/api/types";
import type { SubagentStatus } from "@/features/agents/lib/subagents";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { ManagedAgentRow } from "./ManagedAgentRow";

export type AgentGroupRowsProps = {
  agents: ManagedAgent[];
  channelIdToName: Record<string, string>;
  channelsByPubkey: Record<string, { id: string; name: string }[]>;
  logContent: string | null;
  logError: Error | null;
  logLoading: boolean;
  personaLabelsById: Record<string, string>;
  presenceLoaded: boolean;
  presenceLookup: PresenceLookup;
  selectedLogAgentPubkey: string | null;
  /** Live subagent records (SPEC-nested-subagents); optional, empty default. */
  subagents?: readonly SubagentStatus[];
  onOpenProfile: (pubkey: string) => void;
  onSelectLogAgent: (pubkey: string | null) => void;
};

export function AgentGroupRows({
  agents,
  channelIdToName,
  channelsByPubkey,
  logContent,
  logError,
  logLoading,
  personaLabelsById,
  presenceLoaded,
  presenceLookup,
  selectedLogAgentPubkey,
  subagents = [],
  onOpenProfile,
  onSelectLogAgent,
}: AgentGroupRowsProps) {
  return (
    <div className="divide-y divide-border/50 border-t border-border/50">
      {agents.map((agent) => (
        <ManagedAgentRow
          agent={agent}
          channelIdToName={channelIdToName}
          channelNames={channelsByPubkey[normalizePubkey(agent.pubkey)] ?? []}
          isLogSelected={selectedLogAgentPubkey === agent.pubkey}
          key={agent.pubkey}
          logContent={
            selectedLogAgentPubkey === agent.pubkey ? logContent : null
          }
          logError={selectedLogAgentPubkey === agent.pubkey ? logError : null}
          logLoading={selectedLogAgentPubkey === agent.pubkey && logLoading}
          personaLabelsById={personaLabelsById}
          presenceLoaded={presenceLoaded}
          presenceLookup={presenceLookup}
          subagents={subagents}
          onOpenProfile={onOpenProfile}
          onSelectLogAgent={onSelectLogAgent}
        />
      ))}
    </div>
  );
}
