import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { Evaluator, Lexer, Parser, data } from "@actions/expressions";
import { parse } from "yaml";

const root = fileURLToPath(new URL("../..", import.meta.url));
const workflow = parse(
  readFileSync(path.join(root, ".github/workflows/ci.yml"), "utf8"),
);
const filterSteps = workflow.jobs.changes.steps.filter(
  (step) => step.id === "filter",
);
assert.equal(filterSteps.length, 1);
const filter = filterSteps[0];
const actionRef = "dorny/paths-filter@ceb8a2b8f2d89434be7ff52d3de7ec3738c5cc9d";
const actionSha256 =
  "d7c109e4a3c9f256aab1baf62473673a2e36d15efe02848220001de80067d5b2";
const scratch = mkdtempSync(path.join(tmpdir(), "buzz-ci-paths-filter-"));
const actionPath = path.join(scratch, "paths-filter.cjs");

after(() => rmSync(scratch, { recursive: true, force: true }));
before(async () => {
  assert.equal(
    filter.uses,
    actionRef,
    "review and qualify action updates explicitly",
  );
  assert.equal(filter.with.token, "", "exercise the production Git diff path");
  const revision = actionRef.split("@")[1];
  const response = await fetch(
    `https://raw.githubusercontent.com/dorny/paths-filter/${revision}/dist/index.js`,
    { signal: AbortSignal.timeout(30_000) },
  );
  assert.equal(response.status, 200, "pinned action download must succeed");
  const bytes = Buffer.from(await response.arrayBuffer());
  assert.equal(createHash("sha256").update(bytes).digest("hex"), actionSha256);
  writeFileSync(actionPath, bytes);
});

function outputValues(file) {
  const lines = readFileSync(file, "utf8").split(/\r?\n/);
  const result = {};
  for (let index = 0; index < lines.length; index++) {
    if (!lines[index]) continue;
    const match = /^([^<]+)<<(.+)$/.exec(lines[index]);
    assert.ok(match, `expected an Actions multiline output: ${lines[index]}`);
    const values = [];
    while (++index < lines.length && lines[index] !== match[2])
      values.push(lines[index]);
    assert.ok(index < lines.length, "output delimiter must close");
    assert.ok(!(match[1] in result), "output must be written once");
    result[match[1]] = values.join("\n");
  }
  return result;
}

// Execute the unmodified action bundle, including its Git diff and matcher,
// with the production filter input. No API token or caller credentials enter
// this disposable repository, and Git cannot read the user's global config.
function runAction(change) {
  const directory = mkdtempSync(path.join(scratch, "repo-"));
  const gitConfig = path.join(directory, "fixture-git-config");
  const output = path.join(directory, "action-output");
  const event = path.join(directory, "event.json");
  writeFileSync(gitConfig, "");
  writeFileSync(output, "");
  const env = {
    PATH: process.env.PATH,
    GIT_CONFIG_GLOBAL: gitConfig,
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_TERMINAL_PROMPT: "0",
    GIT_AUTHOR_NAME: "Path filter fixture",
    GIT_AUTHOR_EMAIL: "fixture@example.invalid",
    GIT_COMMITTER_NAME: "Path filter fixture",
    GIT_COMMITTER_EMAIL: "fixture@example.invalid",
  };
  const git = (...args) =>
    execFileSync("git", ["-c", "core.hooksPath=/dev/null", ...args], {
      cwd: directory,
      env,
      encoding: "utf8",
      timeout: 10_000,
      stdio: ["ignore", "pipe", "pipe"],
    }).trim();
  git("-c", "init.templateDir=", "init", "--initial-branch=main");
  for (const name of ["Justfile", "README.md"]) {
    writeFileSync(
      path.join(directory, name),
      readFileSync(path.join(root, name)),
    );
  }
  git("add", "Justfile", "README.md");
  git("commit", "-m", "Base fixture");
  const base = git("rev-parse", "HEAD");
  change(directory);
  git(
    "add",
    "-A",
    "--",
    ".",
    ":!fixture-git-config",
    ":!action-output",
    ":!event.json",
  );
  git("commit", "-m", "Changed fixture");
  const head = git("rev-parse", "HEAD");
  writeFileSync(
    event,
    JSON.stringify({ pull_request: { base: { sha: base } } }),
  );
  try {
    execFileSync(process.execPath, [actionPath], {
      cwd: directory,
      env: {
        ...env,
        GITHUB_EVENT_NAME: "pull_request",
        GITHUB_EVENT_PATH: event,
        GITHUB_OUTPUT: output,
        GITHUB_WORKSPACE: directory,
        GITHUB_REPOSITORY: "fixture/paths",
        GITHUB_REF: "refs/heads/main",
        GITHUB_SHA: head,
        ...Object.fromEntries(
          Object.entries(filter.with).map(([name, value]) => [
            `INPUT_${name.toUpperCase().replace(/ /g, "_")}`,
            String(value),
          ]),
        ),
      },
      timeout: 20_000,
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (error) {
    throw new Error(
      `Pinned paths-filter failed:\n${error.stdout}\n${error.stderr}`,
      { cause: error },
    );
  }
  return outputValues(output);
}

const rustLanes = [
  "rust",
  "rust-cross-compile-domain",
  "desktop-domain",
  "relay-artifacts-domain",
  "postgres-domain",
  "desktop-macos-domain",
  "relay-domain",
  "security-domain",
];

function selectedLanes(outputs) {
  const context = {
    github: { event_name: "pull_request" },
    needs: { changes: { outputs } },
  };
  // Evaluate the existing workflow expressions; this does not execute any job
  // or certify a hosted check, runner environment, or downstream test result.
  return [...rustLanes, "clients", "mobile-swift-domain"].filter((name) => {
    const { tokens } = new Lexer(workflow.jobs[name].if).lex();
    const expression = new Parser(tokens, Object.keys(context), []).parse();
    return (
      new Evaluator(
        expression,
        JSON.parse(JSON.stringify(context), data.reviver),
      )
        .evaluate()
        .coerceString() === "true"
    );
  });
}

for (const scenario of [
  {
    name: "Justfile-only edit",
    rust: true,
    change: (dir) => {
      writeFileSync(path.join(dir, "Justfile"), "# changed recipe\n");
    },
  },
  {
    name: "Justfile deletion",
    rust: true,
    change: (dir) => {
      rmSync(path.join(dir, "Justfile"));
    },
  },
  {
    name: "unrelated documentation edit",
    rust: false,
    change: (dir) => {
      writeFileSync(path.join(dir, "README.md"), "Documentation change\n");
    },
  },
  {
    name: "nested documentation named Justfile",
    rust: false,
    change: (dir) => {
      mkdirSync(path.join(dir, "docs"));
      writeFileSync(path.join(dir, "docs/Justfile"), "Documentation example\n");
    },
  },
]) {
  test(`${scenario.name} selects Rust CI only for the root Justfile`, (t) => {
    const outputs = runAction(scenario.change);
    assert.equal(outputs.rust, String(scenario.rust));
    for (const name of ["desktop-rust", "web", "mobile"]) {
      assert.equal(outputs[name], "false", `${name} must remain unchanged`);
    }
    assert.equal(JSON.parse(outputs.changes).includes("rust"), scenario.rust);
    const lanes = selectedLanes(outputs);
    if (scenario.rust) {
      assert.deepEqual(lanes, rustLanes);
    } else {
      // The existing desktop negation independently over-selects docs under
      // the action's default "some" quantifier. Do not bless that behavior or
      // claim this case-only repair fixes it: verify the Rust-only lanes here.
      for (const name of [
        "rust",
        "rust-cross-compile-domain",
        "postgres-domain",
        "security-domain",
      ]) {
        assert.ok(
          !lanes.includes(name),
          `${name} must not run for documentation`,
        );
      }
    }
    t.diagnostic(
      JSON.stringify({ action: actionRef, actionSha256, outputs, lanes }),
    );
  });
}
