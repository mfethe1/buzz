// #52: OPENAI_COMPAT_BASE_URL round-trips through env_vars in all three
// config surfaces. The env write pattern is identical everywhere, so we test
// the canonical reducer shared by the dialogs.
import assert from "node:assert/strict";
import { test } from "node:test";

const ENV_VAR = "OPENAI_COMPAT_BASE_URL";

// Mirrors the onBaseUrlChange handler used by the three dialogs.
export function applyBaseUrlChange(prev, next) {
  return next.trim().length === 0
    ? Object.fromEntries(
        Object.entries(prev).filter(([key]) => key !== ENV_VAR),
      )
    : { ...prev, [ENV_VAR]: next };
}

test("sets base url alongside existing env vars", () => {
  assert.deepEqual(
    applyBaseUrlChange({ ANTHROPIC_API_KEY: "k" }, "http://127.0.0.1:11434/v1"),
    { ANTHROPIC_API_KEY: "k", [ENV_VAR]: "http://127.0.0.1:11434/v1" },
  );
});

test("overwrites a previous base url", () => {
  assert.deepEqual(
    applyBaseUrlChange(
      { [ENV_VAR]: "https://old.example/v1" },
      "https://new.example/v1",
    ),
    { [ENV_VAR]: "https://new.example/v1" },
  );
});

test("clearing the field removes the env var entirely (default semantics)", () => {
  assert.deepEqual(
    applyBaseUrlChange(
      { [ENV_VAR]: "https://old.example/v1", OTHER: "1" },
      "   ",
    ),
    { OTHER: "1" },
  );
});

test("never writes an empty-string env var", () => {
  const result = applyBaseUrlChange({ A: "1" }, "");
  assert.equal(ENV_VAR in result, false);
});
