"use strict";

const { execFileSync } = require("node:child_process");

const SHA = /^[0-9a-f]{40}$/;
const MAX_COMMITS = 250; // GitHub's pull-request commits endpoint is capped here.

function sameIdentity(actual, expected, repository, number) {
  if (
    actual.number !== number ||
    actual.base?.repo?.full_name !== repository ||
    actual.base?.sha !== expected.base?.sha ||
    actual.head?.sha !== expected.head?.sha ||
    actual.base?.ref !== expected.base?.ref ||
    actual.state !== "open"
  ) {
    throw new Error("Pull request identity changed; rerun for its current base and head");
  }
}

/** Require a Git-parsed sign-off matching the immutable commit author. */
function hasAuthorSignoff(commit) {
  const { message, author } = commit;
  if (
    typeof message !== "string" || Buffer.byteLength(message) > 1024 * 1024 ||
    typeof author?.name !== "string" || !author.name.trim() ||
    typeof author?.email !== "string" || !author.email.trim()
  ) {
    return false;
  }
  const gitEnvironment = { ...process.env, GIT_CONFIG_NOSYSTEM: "1",
    GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_COUNT: "0", GIT_DIR: "/dev/null" };
  delete gitEnvironment.GIT_CONFIG_PARAMETERS;
  const trailers = execFileSync("git", ["interpret-trailers", "--parse"], {
    input: message,
    encoding: "utf8",
    timeout: 5000,
    maxBuffer: 1024 * 1024,
    // Repository configuration must not redefine what counts as a sign-off.
    env: gitEnvironment,
  });
  return trailers.split("\n").some((line) => {
    const match = /^Signed-off-by:\s+([^<>\r\n]+)\s+<([^<>\s]+)>\s*$/i.exec(line);
    return match &&
      match[1].trim().toLowerCase() === author.name.trim().toLowerCase() &&
      match[2].toLowerCase() === author.email.trim().toLowerCase();
  });
}

/** Verify every PR commit using read-only API metadata, fenced by base and head. */
async function verify({ github, context }) {
  if (context.eventName !== "pull_request_target") {
    throw new Error("Product DCO requires a trusted pull_request_target event");
  }
  const repository = `${context.repo.owner}/${context.repo.repo}`;
  const expected = context.payload.pull_request;
  const number = context.payload.number;
  if (
    !Number.isSafeInteger(number) || number < 1 ||
    context.payload.repository?.full_name !== repository ||
    !SHA.test(expected?.base?.sha) || !SHA.test(expected?.head?.sha) ||
    !SHA.test(context.evaluatorSha) || context.evaluatorSha !== expected.base.sha
  ) {
    throw new Error("Missing exact repository, PR, base, head, or trusted evaluator SHA");
  }
  const request = { ...context.repo, pull_number: number };
  const { data: pull } = await github.rest.pulls.get(request);
  sameIdentity(pull, expected, repository, number);
  if (!Number.isInteger(pull.commits) || pull.commits < 1 || pull.commits > MAX_COMMITS) {
    throw new Error("Cannot prove the complete commit list: PR must contain 1–250 commits");
  }
  const commits = await github.paginate(github.rest.pulls.listCommits, {
    ...request, per_page: 100,
  });
  if (
    commits.length !== pull.commits ||
    new Set(commits.map((item) => item.sha)).size !== pull.commits ||
    commits.some((item) => !SHA.test(item.sha)) ||
    commits.at(-1)?.sha !== expected.head.sha
  ) {
    throw new Error("Incomplete, duplicate, or stale pull-request commit list");
  }
  const failures = commits.filter((item) => !hasAuthorSignoff(item.commit)).map((item) => item.sha);
  // A force-push or base update while paginating must invalidate this result.
  const { data: latest } = await github.rest.pulls.get(request);
  sameIdentity(latest, expected, repository, number);
  if (latest.commits !== pull.commits) {
    throw new Error("Pull request commit count changed during verification");
  }
  return {
    schema_version: 1,
    kind: "dco_qualification",
    repository,
    pull_request: number,
    base_sha: expected.base.sha,
    head_sha: expected.head.sha,
    evaluator_sha: context.evaluatorSha,
    commits: commits.map((item) => item.sha),
    qualified: failures.length === 0,
    missing_author_signoff: failures,
  };
}

module.exports = { hasAuthorSignoff, verify };
