import * as React from "react";

import type { TimelineMessage } from "@/features/messages/types";

const ANNOUNCEMENT_COALESCE_MS = 500;
/**
 * How long an announcement stays in the DOM after it is emitted. Long enough
 * for assistive tech to pick the change up, short enough that the sr-only node
 * is not a permanent duplicate of the timeline.
 */
const ANNOUNCEMENT_RETENTION_MS = 500;

type TimelineAnnouncementPolicyState = {
  channelId: string;
  hydrated: boolean;
  messageKeys: string[];
};

type TimelineAnnouncementPolicyInput = {
  channelId: string;
  isHydrated: boolean;
  messages: readonly TimelineMessage[];
};

type TimelineAnnouncementPolicyResult = {
  announcements: string[];
  didReset: boolean;
  state: TimelineAnnouncementPolicyState;
};

type AnnouncementScheduler = {
  clear(timerId: ReturnType<typeof setTimeout>): void;
  schedule(callback: () => void, delay: number): ReturnType<typeof setTimeout>;
};

type TimelineAnnouncementBatcher = {
  dispose(): void;
  push(announcements: readonly string[]): void;
  reset(): void;
};

const defaultScheduler: AnnouncementScheduler = {
  clear: (timerId) => clearTimeout(timerId),
  schedule: (callback, delay) => setTimeout(callback, delay),
};

function messageKey(message: TimelineMessage): string {
  return message.renderKey ?? message.id;
}

/**
 * Replace every `||spoiler||` span with a neutral placeholder.
 *
 * A live region announces the message body verbatim, so an un-redacted
 * announcement hands the spoiler's contents — including the destination of a
 * masked `[label](url)` link — to assistive tech (and to the accessibility
 * tree, where it is trivially readable) before the reader chooses to reveal
 * it. Screen-reader users must get the same "hidden until revealed" guarantee
 * sighted users get, so redact at the announcement source rather than relying
 * on the visual spoiler overlay.
 *
 * Matches the delimiter pair non-greedily and only within a single
 * announcement, mirroring the inline spoiler mark's own parsing.
 */
export function redactSpoilers(body: string): string {
  return body.replace(/\|\|([\s\S]*?)\|\|/g, "spoiler hidden");
}

function announcementForMessage(message: TimelineMessage): string | null {
  const author = message.author.trim();
  const body = redactSpoilers(message.body).replace(/\s+/g, " ").trim();
  if (!author || !body) return null;
  return `${message.isAgent ? "Agent " : ""}${author}: ${body}`;
}

function policyState(
  channelId: string,
  hydrated: boolean,
  messages: readonly TimelineMessage[],
): TimelineAnnouncementPolicyState {
  return {
    channelId,
    hydrated,
    messageKeys: hydrated ? messages.map(messageKey) : [],
  };
}

/**
 * Classifies message-array changes without touching the DOM or timers.
 *
 * A channel's first hydrated snapshot is only a baseline. After that baseline,
 * only a strict append to the known tail is announceable; prepends, replacement
 * snapshots, edits, and reorders are silently re-seeded as history.
 */
export function advanceTimelineAnnouncementPolicy(
  previous: TimelineAnnouncementPolicyState | null,
  input: TimelineAnnouncementPolicyInput,
): TimelineAnnouncementPolicyResult {
  if (!previous || previous.channelId !== input.channelId) {
    return {
      announcements: [],
      didReset: true,
      state: policyState(input.channelId, input.isHydrated, input.messages),
    };
  }

  if (!previous.hydrated) {
    if (!input.isHydrated) {
      return { announcements: [], didReset: false, state: previous };
    }
    return {
      announcements: [],
      didReset: false,
      state: policyState(input.channelId, true, input.messages),
    };
  }

  if (!input.isHydrated) {
    return { announcements: [], didReset: false, state: previous };
  }

  const nextKeys = input.messages.map(messageKey);
  const isTailAppend =
    nextKeys.length > previous.messageKeys.length &&
    previous.messageKeys.every((key, index) => nextKeys[index] === key);

  const announcements = isTailAppend
    ? input.messages
        .slice(previous.messageKeys.length)
        .map(announcementForMessage)
        .filter((announcement): announcement is string => announcement !== null)
    : [];

  return {
    announcements,
    didReset: false,
    state: {
      channelId: input.channelId,
      hydrated: true,
      messageKeys: nextKeys,
    },
  };
}

export function createTimelineAnnouncementBatcher({
  emit,
  scheduler = defaultScheduler,
}: {
  emit: (announcement: string) => void;
  scheduler?: AnnouncementScheduler;
}): TimelineAnnouncementBatcher {
  let pending: string[] = [];
  let timerId: ReturnType<typeof setTimeout> | null = null;

  const reset = () => {
    if (timerId !== null) scheduler.clear(timerId);
    timerId = null;
    pending = [];
  };

  return {
    dispose: reset,
    push(announcements) {
      if (announcements.length === 0) return;
      pending.push(...announcements);
      if (timerId !== null) return;
      timerId = scheduler.schedule(() => {
        timerId = null;
        const announcement = pending.join("; ");
        pending = [];
        if (announcement) emit(announcement);
      }, ANNOUNCEMENT_COALESCE_MS);
    },
    reset,
  };
}

export function TimelineAnnouncementRegion({
  channelId,
  isHydrated,
  messages,
  scheduler = defaultScheduler,
}: {
  channelId: string;
  isHydrated: boolean;
  messages: readonly TimelineMessage[];
  scheduler?: AnnouncementScheduler;
}) {
  const [announcement, setAnnouncement] = React.useState("");
  const policyRef = React.useRef<TimelineAnnouncementPolicyState | null>(null);
  const batcherRef = React.useRef<TimelineAnnouncementBatcher | null>(null);
  const retentionRef = React.useRef<ReturnType<
    typeof scheduler.schedule
  > | null>(null);
  // The scheduler is swapped only by tests; mirroring it in a ref keeps the
  // effects below free of a dependency that would re-run them on every render.
  const schedulerRef = React.useRef(scheduler);
  schedulerRef.current = scheduler;

  const clearRetention = React.useCallback(() => {
    if (retentionRef.current === null) return;
    schedulerRef.current.clear(retentionRef.current);
    retentionRef.current = null;
  }, []);

  if (!batcherRef.current) {
    batcherRef.current = createTimelineAnnouncementBatcher({
      emit: (nextAnnouncement) => {
        setAnnouncement((previousAnnouncement) =>
          previousAnnouncement === nextAnnouncement
            ? `${nextAnnouncement}\u2060`
            : nextAnnouncement,
        );
        // Assistive tech announces on *change*, so the text has done its job
        // once it has been read. Retaining it forever leaves a second copy of
        // every message body in the accessibility tree, where it shadows the
        // real timeline node — `getByText(body)` then resolves to two elements.
        // Drop it again after the retention window.
        clearRetention();
        retentionRef.current = scheduler.schedule(
          () => setAnnouncement(""),
          ANNOUNCEMENT_RETENTION_MS,
        );
      },
      scheduler,
    });
  }

  React.useEffect(() => {
    const result = advanceTimelineAnnouncementPolicy(policyRef.current, {
      channelId,
      isHydrated,
      messages,
    });
    policyRef.current = result.state;

    if (result.didReset) {
      batcherRef.current?.reset();
      clearRetention();
      setAnnouncement("");
    }
    batcherRef.current?.push(result.announcements);
  }, [channelId, clearRetention, isHydrated, messages]);

  React.useEffect(
    () => () => {
      batcherRef.current?.dispose();
      clearRetention();
    },
    [clearRetention],
  );

  return (
    <div
      aria-atomic="true"
      aria-live="polite"
      className="sr-only"
      data-testid="message-timeline-announcements"
      role="status"
    >
      {announcement}
    </div>
  );
}
