import assert from "node:assert/strict";
import test from "node:test";

import {
  opaqueLabelFor,
  saveIsDisabled,
  showsResetControl,
} from "./deviceNameCardLogic.ts";

const DEVICE_ID = "0123456789abcdef0123456789abcdef";

const identity = (deviceLabel) => ({
  deviceId: DEVICE_ID,
  deviceLabel,
  createdAt: "2026-08-27T00:00:00Z",
});

test("the anonymous default mirrors the backend mint rule", () => {
  assert.equal(opaqueLabelFor(DEVICE_ID), "device-01234567");
});

test("Reset is offered once a real name has been published", () => {
  assert.equal(showsResetControl(identity("mfeth-win")), true);
});

test("Reset is withheld on a device already at its default", () => {
  assert.equal(showsResetControl(identity("device-01234567")), false);
});

test("Reset is withheld until the identity has loaded, never rendered disabled", () => {
  assert.equal(showsResetControl(undefined), false);
});

test("a lookalike label the owner typed still gets the way back", () => {
  // Passes the backend label policy and matches `device-[0-9a-f]{8}`, but is
  // not THIS device's default, so hiding Reset would strand the owner.
  assert.equal(showsResetControl(identity("device-deadbeef")), true);
});

test("an empty label is not mistaken for the anonymous default", () => {
  assert.equal(showsResetControl(identity("")), true);
});

const save = (overrides) =>
  saveIsDisabled({
    trimmedLabel: "mfeth-win",
    savedLabel: "device-01234567",
    identityLoaded: true,
    renamePending: false,
    resetPending: false,
    ...overrides,
  });

test("Save is enabled for a new, non-empty, loaded label", () => {
  assert.equal(save({}), false);
});

test("Save is disabled while a reset is in flight", () => {
  // Both writes go to the same device.json and both republish; concurrently
  // the published label is whichever finished last.
  assert.equal(save({ resetPending: true }), true);
});

test("Save is disabled while a rename is in flight", () => {
  assert.equal(save({ renamePending: true }), true);
});

test("Save is disabled for an empty or unchanged label", () => {
  assert.equal(save({ trimmedLabel: "" }), true);
  assert.equal(save({ trimmedLabel: "device-01234567" }), true);
});

test("Save is disabled before the identity loads", () => {
  assert.equal(save({ identityLoaded: false }), true);
});
