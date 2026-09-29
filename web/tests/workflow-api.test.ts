import assert from "node:assert/strict";
import { test } from "node:test";
import {
  createWorkflow,
  listRuns,
  listWorkflows,
  triggerWorkflow,
  workflowSummary,
} from "../src/features/workflows/workflow-api";

const channel = "11111111-1111-4111-8111-111111111111";
const manual = "name: Check\ntrigger:\n  on: manual\nsteps: []";

test("workflow YAML uses the same trigger keys as the relay", () => {
  assert.deepEqual(workflowSummary(manual), {
    name: "Check",
    manual: true,
    webhook: false,
    enabled: true,
    valid: true,
  });
  assert.equal(workflowSummary("trigger:\n  on: webhook").webhook, true);
  assert.equal(
    workflowSummary("trigger:\n  on: manual\ntrigger:\n  on: webhook").valid,
    false,
  );
});

test("fail closed before a network request or signer prompt", async () => {
  await assert.rejects(listWorkflows("not-a-channel"), /valid channel UUID/);
  await assert.rejects(listRuns("not-an-id"), /Invalid workflow id/);
  await assert.rejects(
    createWorkflow(channel, "trigger:\n  on: webhook"),
    /Webhook creation/,
  );
  await assert.rejects(
    createWorkflow(channel, "trigger: [invalid"),
    /Invalid workflow YAML/,
  );
  await assert.rejects(
    triggerWorkflow(
      {
        id: channel,
        owner: "another-user",
        channelId: channel,
        revision: "",
        yaml: manual,
      },
      "me",
    ),
    /Only the workflow owner/,
  );
  await assert.rejects(
    triggerWorkflow(
      {
        id: channel,
        owner: "me",
        channelId: channel,
        revision: "",
        yaml: "name: Disabled\nenabled: false\ntrigger:\n  on: manual",
      },
      "me",
    ),
    /Only enabled manual/,
  );
});
