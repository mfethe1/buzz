import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";

import { JSDOM } from "jsdom";

import {
  TimelineAnnouncementRegion,
  advanceTimelineAnnouncementPolicy,
  createTimelineAnnouncementBatcher,
  redactSpoilers,
} from "./TimelineAnnouncementRegion.tsx";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});

Object.assign(globalThis, {
  IS_REACT_ACT_ENVIRONMENT: true,
  document: dom.window.document,
  self: dom.window,
  window: dom.window,
});
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: dom.window.navigator,
});

let act;
let cleanup;
let createElement;
let render;

before(async () => {
  ({ act, cleanup, render } = await import("@testing-library/react"));
  ({ createElement } = await import("react"));
});

afterEach(() => {
  cleanup();
  document.body.replaceChildren();
});

after(() => dom.window.close());

function message(id, author, body, extra = {}) {
  return {
    author,
    body,
    createdAt: 1,
    depth: 0,
    id,
    time: "12:00",
    ...extra,
  };
}

function advance(state, channelId, messages, isHydrated = true) {
  return advanceTimelineAnnouncementPolicy(state, {
    channelId,
    isHydrated,
    messages,
  });
}

function manualScheduler() {
  let nextId = 1;
  const tasks = new Map();
  return {
    clear(id) {
      tasks.delete(id);
    },
    pendingCount() {
      return tasks.size;
    },
    runAll() {
      const callbacks = [...tasks.values()];
      tasks.clear();
      for (const callback of callbacks) callback();
    },
    schedule(callback, delay) {
      assert.equal(delay, 500);
      const id = nextId++;
      tasks.set(id, callback);
      return id;
    },
  };
}

test("initial history hydrates silently before tail announcements begin", () => {
  let result = advance(null, "alpha", [], false);
  assert.deepEqual(result.announcements, []);

  result = advance(
    result.state,
    "alpha",
    [message("old-1", "Alice", "Earlier"), message("old-2", "Bob", "History")],
    true,
  );
  assert.deepEqual(result.announcements, []);
});

test("one incoming tail message produces one concise announcement", () => {
  const seeded = advance(null, "alpha", [message("old", "Alice", "Earlier")]);
  const result = advance(seeded.state, "alpha", [
    message("old", "Alice", "Earlier"),
    message("new", "Bob", "Deploy finished"),
  ]);

  assert.deepEqual(result.announcements, ["Bob: Deploy finished"]);
});

test("history prepends and replacement snapshots remain silent", () => {
  const seeded = advance(null, "alpha", [message("newer", "Bob", "Current")]);
  const prepended = advance(seeded.state, "alpha", [
    message("older", "Alice", "Loaded history"),
    message("newer", "Bob", "Current"),
  ]);
  assert.deepEqual(prepended.announcements, []);

  const replaced = advance(prepended.state, "alpha", [
    message("replacement", "Alice", "Replacement snapshot"),
  ]);
  assert.deepEqual(replaced.announcements, []);
});

test("agent action announces its semantic body without hidden tool trace data", () => {
  const seeded = advance(null, "alpha", []);
  const result = advance(seeded.state, "alpha", [
    message("agent-1", "Pollen", "Ran tests: 42 passed", {
      isAgent: true,
      tags: [["tool-trace", "SECRET raw command output"]],
    }),
  ]);

  assert.deepEqual(result.announcements, [
    "Agent Pollen: Ran tests: 42 passed",
  ]);
  assert.equal(result.announcements.join(" ").includes("SECRET"), false);
});

test("burst announcements coalesce into one update within 500ms", () => {
  const scheduler = manualScheduler();
  const emitted = [];
  const batcher = createTimelineAnnouncementBatcher({
    emit: (announcement) => emitted.push(announcement),
    scheduler,
  });

  batcher.push(["Alice: First"]);
  batcher.push(["Agent Pollen: Second", "Bob: Third"]);

  assert.equal(scheduler.pendingCount(), 1);
  assert.deepEqual(emitted, []);
  scheduler.runAll();
  assert.deepEqual(emitted, ["Alice: First; Agent Pollen: Second; Bob: Third"]);
});

test("channel reset cancels a pending burst and seeds replacement history silently", () => {
  const scheduler = manualScheduler();
  const emitted = [];
  const batcher = createTimelineAnnouncementBatcher({
    emit: (announcement) => emitted.push(announcement),
    scheduler,
  });
  const alpha = advance(null, "alpha", [message("a1", "Alice", "Earlier")]);
  const alphaAppend = advance(alpha.state, "alpha", [
    message("a1", "Alice", "Earlier"),
    message("a2", "Alice", "Pending"),
  ]);
  batcher.push(alphaAppend.announcements);

  const beta = advance(alphaAppend.state, "beta", [
    message("b1", "Bob", "Existing beta history"),
  ]);
  if (beta.didReset) batcher.reset();

  scheduler.runAll();
  assert.deepEqual(beta.announcements, []);
  assert.deepEqual(emitted, []);
});

test("persistent status region announces without moving focus", async () => {
  const scheduler = manualScheduler();
  const input = document.createElement("input");
  document.body.append(input);
  input.focus();

  const view = render(
    createElement(TimelineAnnouncementRegion, {
      channelId: "alpha",
      isHydrated: true,
      messages: [message("old", "Alice", "Earlier")],
      scheduler,
    }),
  );
  const region = view.getByTestId("message-timeline-announcements");
  assert.equal(region.getAttribute("role"), "status");
  assert.equal(region.getAttribute("aria-live"), "polite");
  assert.equal(region.getAttribute("aria-atomic"), "true");
  assert.equal(region.textContent, "");
  assert.equal(document.activeElement, input);

  await act(async () => {
    view.rerender(
      createElement(TimelineAnnouncementRegion, {
        channelId: "alpha",
        isHydrated: true,
        messages: [
          message("old", "Alice", "Earlier"),
          message("new", "Bob", "New arrival"),
        ],
        scheduler,
      }),
    );
  });
  assert.equal(view.getAllByRole("status").length, 1);

  await act(async () => scheduler.runAll());
  assert.equal(region.textContent, "Bob: New arrival");
  assert.equal(document.activeElement, input);
});

test("redactSpoilers hides spoiler contents from announcements", () => {
  // A masked link inside a spoiler must not leak its destination.
  assert.equal(
    redactSpoilers("||[secret](https://private.example/leak-path)||"),
    "spoiler hidden",
  );
  // Surrounding text is preserved; only the spoiler span is replaced.
  assert.equal(
    redactSpoilers("before ||hidden|| after"),
    "before spoiler hidden after",
  );
  // Multiple spoilers are each redacted independently (non-greedy).
  assert.equal(
    redactSpoilers("||one|| middle ||two||"),
    "spoiler hidden middle spoiler hidden",
  );
  // Text with no spoiler is untouched.
  assert.equal(redactSpoilers("plain body"), "plain body");
  // An unpaired delimiter is not a spoiler and must not be swallowed.
  assert.equal(redactSpoilers("just || dangling"), "just || dangling");
});

test("announcements redact spoilers before they reach the live region", async () => {
  const scheduler = manualScheduler();
  const view = render(
    createElement(TimelineAnnouncementRegion, {
      channelId: "alpha",
      isHydrated: true,
      messages: [message("old", "Alice", "Earlier")],
      scheduler,
    }),
  );
  const region = view.getByTestId("message-timeline-announcements");

  await act(async () => {
    view.rerender(
      createElement(TimelineAnnouncementRegion, {
        channelId: "alpha",
        isHydrated: true,
        messages: [
          message("old", "Alice", "Earlier"),
          message(
            "new",
            "Bob",
            "||[secret](https://private.example/leak-path)||",
          ),
        ],
        scheduler,
      }),
    );
  });
  await act(async () => scheduler.runAll());

  assert.equal(region.textContent, "Bob: spoiler hidden");
  assert.ok(!region.textContent.includes("private.example"));
});

test("the live region clears after its retention window", async () => {
  const scheduler = manualScheduler();
  const view = render(
    createElement(TimelineAnnouncementRegion, {
      channelId: "alpha",
      isHydrated: true,
      messages: [message("old", "Alice", "Earlier")],
      scheduler,
    }),
  );
  const region = view.getByTestId("message-timeline-announcements");

  await act(async () => {
    view.rerender(
      createElement(TimelineAnnouncementRegion, {
        channelId: "alpha",
        isHydrated: true,
        messages: [
          message("old", "Alice", "Earlier"),
          message("new", "Bob", "Toggle me"),
        ],
        scheduler,
      }),
    );
  });
  // Coalesce window fires: the announcement is published.
  await act(async () => scheduler.runAll());
  assert.equal(region.textContent, "Bob: Toggle me");

  // Retention window fires: the text must not linger as a permanent duplicate
  // of the timeline node, which would make getByText(body) ambiguous.
  await act(async () => scheduler.runAll());
  assert.equal(region.textContent, "");
});
