import * as React from "react";

import { useChannelsQuery } from "@/features/channels/hooks";
import { ChannelTaskList } from "@/features/tasks/ui/ChannelTaskList";
import { useIdentityQuery } from "@/shared/api/hooks";
import { Button } from "@/shared/ui/button";

type TaskView = "tasks" | "requests";

/**
 * Community-wide task view with explicit channel and owner-request scopes.
 * Request rows retain transport provenance while the assignee filter keeps the
 * owner view tied to the active Buzz identity.
 */
export function TasksScreen() {
  const channels = useChannelsQuery().data ?? [];
  const identityQuery = useIdentityQuery();
  const [activeView, setActiveView] = React.useState<TaskView>("tasks");
  const [selectedChannelId, setSelectedChannelId] = React.useState<
    string | null
  >(null);
  const channelNamesById = React.useMemo(
    () => new Map(channels.map((channel) => [channel.id, channel.name])),
    [channels],
  );
  const sortedChannels = React.useMemo(
    () =>
      [...channels].sort((left, right) => left.name.localeCompare(right.name)),
    [channels],
  );

  React.useEffect(() => {
    if (
      selectedChannelId !== null &&
      !channels.some((channel) => channel.id === selectedChannelId)
    ) {
      setSelectedChannelId(null);
    }
  }, [channels, selectedChannelId]);

  return (
    <div className="flex h-full flex-col gap-4 overflow-y-auto p-4">
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="mb-2 flex flex-wrap items-center gap-2">
            <h1 className="mr-2 text-lg font-semibold">Channel tasks</h1>
            <Button
              data-testid="all-tasks-view"
              onClick={() => setActiveView("tasks")}
              size="sm"
              type="button"
              variant={activeView === "tasks" ? "secondary" : "ghost"}
            >
              All tasks
            </Button>
            <Button
              data-testid="my-requests-view"
              onClick={() => setActiveView("requests")}
              size="sm"
              type="button"
              variant={activeView === "requests" ? "secondary" : "ghost"}
            >
              My requests
            </Button>
          </div>
          <p className="text-sm text-muted-foreground">
            {activeView === "requests"
              ? "Requests assigned to you, with their original transport and lifecycle state."
              : "Review every channel together or focus on one channel. Changes are visible to every member and on mobile."}
          </p>
        </div>
        <label className="flex min-w-48 flex-col gap-1 text-xs font-medium text-muted-foreground">
          Channel
          <select
            className="h-9 rounded-md border border-input bg-background px-3 text-sm text-foreground shadow-xs outline-none focus-visible:ring-2 focus-visible:ring-ring"
            data-testid="channel-task-scope"
            onChange={(event) =>
              setSelectedChannelId(event.target.value || null)
            }
            value={selectedChannelId ?? ""}
          >
            <option value="">All channels</option>
            {sortedChannels.map((channel) => (
              <option key={channel.id} value={channel.id}>
                #{channel.name}
              </option>
            ))}
          </select>
        </label>
      </header>
      <ChannelTaskList
        assigneePubkey={
          activeView === "requests" ? identityQuery.data?.pubkey : null
        }
        channelId={selectedChannelId}
        channelNamesById={channelNamesById}
        requestsOnly={activeView === "requests"}
        showComposer={activeView !== "requests"}
      />
    </div>
  );
}
