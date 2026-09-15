// #51: persistent action-error banner reducer.
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  actionBannerInitial,
  actionBannerReduce,
} from "./actionBannerState.ts";

test("sync keeps a new error message visible", () => {
  const next = actionBannerReduce(actionBannerInitial, {
    type: "sync",
    errorMessage: 'command not found on PATH: "buzz-acp"',
    noticeMessage: null,
  });
  assert.equal(next.error, 'command not found on PATH: "buzz-acp"');
});

test("sync with an identical error does not churn state", () => {
  const prev = { error: "boom" };
  const next = actionBannerReduce(prev, {
    type: "sync",
    errorMessage: "boom",
    noticeMessage: null,
  });
  assert.equal(next, prev);
});

test("sync clears the banner when a notice arrives (successful action)", () => {
  const next = actionBannerReduce(
    { error: "boom" },
    {
      type: "sync",
      errorMessage: null,
      noticeMessage: "Agent started",
    },
  );
  assert.equal(next.error, null);
});

test("sync with no messages leaves the banner untouched", () => {
  const prev = { error: "boom" };
  const next = actionBannerReduce(prev, {
    type: "sync",
    errorMessage: null,
    noticeMessage: null,
  });
  assert.equal(next, prev);
});

test("dismiss clears the banner", () => {
  const next = actionBannerReduce({ error: "boom" }, { type: "dismiss" });
  assert.equal(next.error, null);
});

test("dismiss on an empty banner is a no-op (stable reference)", () => {
  const next = actionBannerReduce(actionBannerInitial, { type: "dismiss" });
  assert.equal(next, actionBannerInitial);
});
