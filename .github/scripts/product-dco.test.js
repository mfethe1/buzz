"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const { hasAuthorSignoff, verify } = require("./product-dco.js");

const BASE = "a".repeat(40);
const HEAD = "b".repeat(40);
const TESTED = "c".repeat(40);
const author = { name: "Test Author", email: "author@example.com" };
const signed = "Example change\n\nSigned-off-by: Test Author <author@example.com>\n";

function harness(count = 1) {
  const context = {
    repo: { owner: "mfethe1", repo: "buzz" },
    sha: BASE,
    evaluatorSha: BASE,
    eventName: "pull_request_target",
    payload: {
      repository: { full_name: "mfethe1/buzz" },
      number: 19,
      pull_request: {
        number: 19, state: "open", commits: count,
        base: { sha: BASE, ref: "product/main", repo: { full_name: "mfethe1/buzz" } },
        head: { sha: HEAD },
      },
    },
  };
  const initial = structuredClone(context.payload.pull_request);
  const latest = structuredClone(initial);
  const commits = Array.from({ length: count }, (_, index) => ({
    sha: index === count - 1 ? HEAD : (index + 1).toString(16).padStart(40, "0"),
    commit: { author, message: signed },
  }));
  let reads = 0;
  const calls = [];
  const listCommits = () => { throw new Error("Must paginate listCommits"); };
  const github = {
    rest: { pulls: {
      get: async (request) => {
        calls.push(request);
        return { data: reads++ === 0 ? initial : latest };
      },
      listCommits,
    } },
    paginate: async (method, request) => {
      assert.equal(method, listCommits);
      assert.equal(request.per_page, 100);
      calls.push(request);
      return commits;
    },
  };
  return { github, context, initial, latest, commits, calls };
}

test("Git parses a real matching author sign-off with additional trailers", () => {
  assert.ok(hasAuthorSignoff({ author, message: `${signed}Reviewed-by: Another Person <other@example.com>\n` }));
  assert.ok(hasAuthorSignoff({ author, message: signed.replace("Test Author", "test author") }));
});

test("body text, non-author sign-offs, malformed emails, and missing metadata fail", () => {
  for (const message of [
    "Example change", signed.replace("author@example.com", "other@example.com"),
    signed.replace("Test Author", "Different Author"),
    signed.replace("<author@example.com>", "author@example.com"),
    `${signed}\nThat was a quoted example, not my sign-off.`,
    "> Signed-off-by: Test Author <author@example.com>\n",
  ]) {
    assert.equal(Boolean(hasAuthorSignoff({ author, message })), false, message);
  }
  assert.equal(hasAuthorSignoff({ author: {}, message: signed }), false);
  assert.equal(hasAuthorSignoff({ author, message: "x".repeat(1024 * 1024 + 1) }), false);
});

test("complete paginated verification binds repo, PR, base, head, and trusted evaluator", async () => {
  const data = harness(101);
  const receipt = await verify(data);
  assert.equal(receipt.qualified, true);
  assert.equal(receipt.commits.length, 101);
  assert.equal(receipt.repository, "mfethe1/buzz");
  assert.equal(receipt.pull_request, 19);
  assert.equal(receipt.base_sha, BASE);
  assert.equal(receipt.head_sha, HEAD);
  assert.equal(receipt.evaluator_sha, BASE);
  assert.equal(data.calls.length, 3);
  assert.ok(data.calls.every((call) => call.owner === "mfethe1" && call.repo === "buzz" && call.pull_number === 19));
});

test("one unsigned middle commit denies the whole PR, including bots and merges", async () => {
  const data = harness(101);
  data.commits[50].commit.message = "Unsigned change";
  data.commits[50].author = { type: "Bot" };
  data.commits[50].parents = [{ sha: BASE }, { sha: TESTED }];
  const receipt = await verify(data);
  assert.equal(receipt.qualified, false);
  assert.deepEqual(receipt.missing_author_signoff, [data.commits[50].sha]);
});

test("a caller cannot authorize the wrong repository or an unpinned commit", async () => {
  for (const mutate of [
    (data) => { data.context.payload.repository.full_name = "other/repo"; },
    (data) => { data.context.eventName = "workflow_dispatch"; },
    (data) => { data.context.evaluatorSha = "product/main"; },
    (data) => { data.context.payload.pull_request.head.sha = "main"; },
  ]) {
    const data = harness(); mutate(data);
    await assert.rejects(verify(data));
    assert.equal(data.calls.length, 0);
  }
});

test("stale head, base, target branch, repository or closed PR deny before listing commits", async () => {
  for (const mutate of [
    (pull) => { pull.head.sha = TESTED; },
    (pull) => { pull.base.sha = TESTED; },
    (pull) => { pull.base.ref = "other-branch"; },
    (pull) => { pull.base.repo.full_name = "other/repo"; },
    (pull) => { pull.state = "closed"; },
    (pull) => { pull.number = 20; },
  ]) {
    const data = harness(); mutate(data.initial);
    await assert.rejects(verify(data), /identity changed/);
    assert.equal(data.calls.length, 1);
  }
});

test("head or base movement during pagination invalidates previously valid sign-offs", async () => {
  for (const side of ["head", "base"]) {
    const data = harness(); data.latest[side].sha = TESTED;
    await assert.rejects(verify(data), /identity changed/);
  }
  const data = harness(); data.latest.commits += 1;
  await assert.rejects(verify(data), /count changed/);
});

test("partial lists, duplicate entries, wrong last commit, and malformed SHAs fail closed", async () => {
  for (const mutate of [
    (data) => { data.commits.splice(1, 1); },
    (data) => { data.commits[0].sha = data.commits[1].sha; },
    (data) => { data.commits.at(-1).sha = TESTED; },
    (data) => { data.commits[0].sha = "main"; },
  ]) {
    const data = harness(3); mutate(data);
    await assert.rejects(verify(data), /Incomplete, duplicate, or stale/);
  }
});

test("API truncation limit and missing count do not become a partial pass", async () => {
  for (const count of [0, 251, undefined, "1"]) {
    const data = harness(); data.initial.commits = count;
    await assert.rejects(verify(data), /complete commit list/);
    assert.equal(data.calls.length, 1);
  }
});

test("provider errors propagate rather than authorize an empty result", async () => {
  const data = harness();
  data.github.paginate = async () => { throw new Error("API unavailable"); };
  await assert.rejects(verify(data), /API unavailable/);
});

const workflow = readFileSync(path.join(__dirname, "../workflows/product-dco.yml"), "utf8");

function workflowScript(name) {
  const section = workflow.split(`      - name: ${name}\n`)[1]?.split("      - name:")[0];
  const body = section?.split("          script: |\n")[1];
  assert.ok(body, `missing runtime script: ${name}`);
  const source = body.split("\n").filter((line) => line.startsWith("            ")).map((line) => line.slice(12)).join("\n");
  return new (Object.getPrototypeOf(async function () {}).constructor)("github", "context", "core", "require", "process", source);
}

test("trusted workflow pins base evaluator and publishes its own exact-head context", () => {
  assert.match(workflow, /^  pull_request_target:$/m);
  assert.doesNotMatch(workflow, /^  pull_request:$/m);
  assert.match(workflow, /ref: \$\{\{ github\.event\.pull_request\.base\.sha \}\}/);
  assert.match(workflow, /path: trusted-dco/);
  assert.doesNotMatch(workflow, /ref:.*head|persist-credentials: true|contents: write|pull-requests: write/);
  assert.ok(workflow.indexOf("Retain exact-change DCO evidence") < workflow.indexOf("Publish final result"));
  const ci = readFileSync(path.join(__dirname, "../workflows/ci.yml"), "utf8").split("  product-qualification:")[1];
  assert.match(ci, /path: trusted-qualification/);
  assert.match(ci, /run: python3 trusted-qualification\/scripts\/product-qualification\.py/);
  assert.match(ci, /QUALIFICATION_EVALUATOR_SHA:.*pull_request\.base\.sha.*github\.event\.before/);
});

test("actual trusted workflow never imports candidate verifier and fails its unsigned head", async () => {
  const data = harness();
  data.commits[0].commit.message = "Unsigned candidate change";
  const files = new Map();
  const updates = [];
  const outputs = new Map();
  const failures = [];
  const core = { setOutput: (key, value) => outputs.set(key, value), setFailed: (message) => failures.push(message) };
  data.github.rest.checks = {
    create: async (request) => {
      assert.equal(request.name, "Product DCO");
      assert.equal(request.head_sha, HEAD);
      assert.equal(request.status, "in_progress");
      return { data: { id: 42 } };
    },
    update: async (request) => updates.push(request),
  };
  const runtime = { env: { RUNNER_TEMP: "/runner-temp", DCO_EVALUATOR_SHA: BASE,
    DCO_CHECK_ID: "42", DCO_JOB_STATUS: "success" } };
  const importTrusted = (name) => {
    if (name === "./trusted-dco/.github/scripts/product-dco.js") return { verify };
    if (name === "node:path") return path;
    if (name === "node:fs") return {
      writeFileSync: (name, content) => files.set(name, content),
      readFileSync: (name) => { if (!files.has(name)) throw Error("missing receipt"); return files.get(name); },
    };
    throw Error(`Candidate or unexpected module import: ${name}`);
  };
  for (const name of ["Start check on the exact candidate head", "Verify every commit's author sign-off",
    "Publish final result after refreshing PR identity"]) {
    await workflowScript(name)(data.github, data.context, core, importTrusted, runtime);
  }
  assert.equal(outputs.get("check_id"), 42);
  assert.equal(updates.length, 1);
  assert.equal(updates[0].conclusion, "failure");
  assert.equal(updates[0].check_run_id, 42);
  assert.ok(failures.length);
});

test("actual publisher rejects stale tuple, absent receipt, wrong evaluator, and failed evidence upload", async () => {
  for (const scenario of ["success", "new-base", "new-head", "missing", "wrong-evaluator", "failed-job"]) {
    const data = harness();
    const receipt = await verify(data);
    const updates = [];
    data.github.rest.checks = { update: async (request) => updates.push(request) };
    if (scenario === "new-base") data.latest.base.sha = TESTED;
    if (scenario === "new-head") data.latest.head.sha = TESTED;
    if (scenario === "wrong-evaluator") receipt.evaluator_sha = HEAD;
    const runtime = { env: { RUNNER_TEMP: "/runner-temp", DCO_CHECK_ID: "42",
      DCO_JOB_STATUS: scenario === "failed-job" ? "failure" : "success" } };
    const importData = (name) => name === "node:path" ? path : {
      readFileSync: () => { if (scenario === "missing") throw Error("missing"); return JSON.stringify(receipt); },
    };
    await workflowScript("Publish final result after refreshing PR identity")(
      data.github, data.context, { setFailed: () => {} }, importData, runtime);
    assert.equal(updates[0].conclusion, scenario === "success" ? "success" : "failure", scenario);
  }
});
