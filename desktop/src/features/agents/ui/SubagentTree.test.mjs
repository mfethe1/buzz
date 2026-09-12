import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});

before(() => {
  Object.assign(globalThis, {
    document: dom.window.document,
    HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
    window: dom.window,
  });
});

after(() => dom.window.close());

const PARENT_MACK = "m".repeat(64);
const PARENT_ROSIE = "r".repeat(64);

function subagent(overrides = {}) {
  return {
    id: "sub-1",
    name: "research-worker",
    parentPubkey: PARENT_MACK,
    status: "running",
    lastActiveAt: 1_000,
    ...overrides,
  };
}

test("subagent rows are default-collapsed and carry a live active-count badge", async () => {
  const { cleanup, render, screen } = await import("@testing-library/react");
  const React = await import("react");
  const { SubagentTree } = await import("./SubagentTree.tsx");

  const children = [
    subagent({ id: "w1", name: "worker-one", status: "running" }),
    subagent({ id: "w2", name: "worker-two", status: "spawned" }),
    subagent({ id: "w3", name: "worker-three", status: "complete" }),
  ];

  try {
    render(
      React.createElement(SubagentTree, {
        parentPubkeys: [PARENT_MACK],
        subagents: children,
      }),
    );

    // Badge counts active (spawned+running) children, not all records.
    assert.match(
      screen.getByTestId(`subagent-active-count-${PARENT_MACK}`).textContent,
      /\(2 active\)/,
    );

    // Default-collapsed: toggle exists, child rows do not.
    assert.ok(screen.getByTestId(`subagent-toggle-${PARENT_MACK}`));
    assert.equal(
      screen.queryByTestId("subagent-row-w1"),
      null,
      "collapsed by default — no child row rendered",
    );
    assert.equal(
      screen
        .getByTestId(`subagent-toggle-${PARENT_MACK}`)
        .getAttribute("aria-expanded"),
      "false",
    );
  } finally {
    cleanup();
  }
});

test("expanding a parent reveals status dot, name, and idle time per child", async () => {
  const { cleanup, fireEvent, render, screen } = await import(
    "@testing-library/react"
  );
  const React = await import("react");
  const { SubagentTree } = await import("./SubagentTree.tsx");

  try {
    render(
      React.createElement(SubagentTree, {
        parentPubkeys: [PARENT_MACK],
        subagents: [
          subagent({ id: "w1", name: "worker-one", status: "running" }),
          subagent({ id: "w2", name: "worker-two", status: "failed" }),
        ],
      }),
    );

    fireEvent.click(screen.getByTestId(`subagent-toggle-${PARENT_MACK}`));

    assert.equal(
      screen
        .getByTestId(`subagent-toggle-${PARENT_MACK}`)
        .getAttribute("aria-expanded"),
      "true",
    );
    const list = screen.getByTestId(`subagent-list-${PARENT_MACK}`);
    assert.match(list.textContent, /worker-one/);
    assert.match(list.textContent, /worker-two/);
    assert.match(list.textContent, /idle \d+(h \d+m \d+s|m \d+s|s)/);
    assert.ok(
      screen.getByTestId("subagent-status-running"),
      "status dot carries its lifecycle status",
    );
    assert.ok(screen.getByTestId("subagent-status-failed"));

    // Collapse again hides the children.
    fireEvent.click(screen.getByTestId(`subagent-toggle-${PARENT_MACK}`));
    assert.equal(screen.queryByTestId(`subagent-list-${PARENT_MACK}`), null);
  } finally {
    cleanup();
  }
});

test("parents without subagent records render no tree, and orphans stay off parent rows", async () => {
  const { cleanup, render, screen } = await import("@testing-library/react");
  const React = await import("react");
  const { SubagentTree } = await import("./SubagentTree.tsx");

  try {
    // No records at all → nothing rendered.
    render(
      React.createElement(SubagentTree, {
        parentPubkeys: [PARENT_MACK],
        subagents: [],
      }),
    );
    assert.equal(screen.queryByTestId("subagent-tree"), null);
    cleanup();

    // Records for an unknown parent → no toggle surfaces on a known parent.
    render(
      React.createElement(SubagentTree, {
        parentPubkeys: [PARENT_MACK],
        subagents: [subagent({ parentPubkey: PARENT_ROSIE })],
      }),
    );
    assert.equal(screen.queryByTestId(`subagent-toggle-${PARENT_MACK}`), null);
    assert.equal(
      screen.queryByTestId("subagent-tree"),
      null,
      "an orphaned child never surfaces on an unrelated parent",
    );
  } finally {
    cleanup();
  }
});

test("siblings of different parents get independent toggles and badges", async () => {
  const { cleanup, fireEvent, render, screen } = await import(
    "@testing-library/react"
  );
  const React = await import("react");
  const { SubagentTree } = await import("./SubagentTree.tsx");

  try {
    render(
      React.createElement(SubagentTree, {
        parentPubkeys: [PARENT_MACK, PARENT_ROSIE],
        subagents: [
          subagent({ id: "m1", parentPubkey: PARENT_MACK }),
          subagent({ id: "r1", parentPubkey: PARENT_ROSIE }),
        ],
      }),
    );

    fireEvent.click(screen.getByTestId(`subagent-toggle-${PARENT_MACK}`));
    assert.ok(screen.getByTestId(`subagent-list-${PARENT_MACK}`));
    // Rosie's nest stays collapsed — one toggle never expands another's.
    assert.equal(screen.queryByTestId(`subagent-list-${PARENT_ROSIE}`), null);
    assert.match(
      screen.getByTestId(`subagent-active-count-${PARENT_ROSIE}`).textContent,
      /\(1 active\)/,
    );
  } finally {
    cleanup();
  }
});
