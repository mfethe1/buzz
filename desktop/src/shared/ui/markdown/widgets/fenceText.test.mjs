import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";

import { widgetFenceText } from "./fenceText.ts";

const code = (children) => React.createElement("code", {}, children);

test("widgetFenceText: reads a single string child", () => {
  assert.equal(widgetFenceText(code('{"v":1}')), '{"v":1}');
});

test("widgetFenceText: rejoins a fragmented fence body", () => {
  assert.equal(
    widgetFenceText(code(['{"v":1,', '"type":', '"table"}'])),
    '{"v":1,"type":"table"}',
  );
});

test("widgetFenceText: descends through nested syntax elements", () => {
  const nested = code([React.createElement("span", {}, '{"v":'), "1}"]);
  assert.equal(widgetFenceText(nested), '{"v":1}');
});

test("widgetFenceText: ignores null and boolean leaves", () => {
  assert.equal(widgetFenceText(code(["a", null, false, "b"])), "ab");
});

test("widgetFenceText: preserves newlines and whitespace exactly", () => {
  assert.equal(widgetFenceText(code(['{\n  "v": 1\n}'])), '{\n  "v": 1\n}');
});
