"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
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
