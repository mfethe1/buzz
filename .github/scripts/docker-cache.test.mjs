import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { Evaluator, Lexer, Parser, data } from "@actions/expressions";
import { parse } from "yaml";

const root = fileURLToPath(new URL("../..", import.meta.url));
const workflow = parse(
  readFileSync(path.join(root, ".github/workflows/docker.yml"), "utf8"),
);

// Use GitHub's parser/evaluator on the actual workflow values. This does not
// substitute a hand-written Boolean model for the policy under test.
function evaluate(expression, context) {
  const { tokens } = new Lexer(expression).lex();
  const parsed = new Parser(tokens, Object.keys(context), []).parse();
  return new Evaluator(
    parsed,
    JSON.parse(JSON.stringify(context), data.reviver),
  )
    .evaluate()
    .coerceString();
}

function expand(value, context) {
  return value
    .replace(/\$\{\{([\s\S]*?)\}\}/g, (_, expression) =>
      evaluate(expression.trim(), context),
    )
    .trim();
}

function step(job, id) {
  const selected = job.steps.filter((candidate) => candidate.id === id);
  assert.equal(selected.length, 1, `exactly one ${id} step`);
  return selected[0];
}

function resolveCache(job, repository) {
  const directory = mkdtempSync(path.join(tmpdir(), "buzz-docker-cache-"));
  const output = path.join(directory, "output");
  try {
    execFileSync("bash", ["-euo", "pipefail", "-c", step(job, "cache").run], {
      env: {
        PATH: process.env.PATH,
        GITHUB_REPOSITORY: repository,
        GITHUB_OUTPUT: output,
      },
    });
    const lines = readFileSync(output, "utf8").trim().split("\n");
    assert.equal(lines.length, 1, "one cache ownership output");
    assert.ok(lines[0].startsWith("repository="));
    return lines[0].slice("repository=".length);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

const configurations = [
  { job: workflow.jobs.build, buildId: "build-release", suffix: "-buildcache" },
  {
    job: workflow.jobs["push-gateway-build"],
    buildId: "build",
    suffix: "-push-gateway-buildcache",
  },
];

for (const { job, buildId, suffix } of configurations) {
  test(`${job.name}: cache owner is this repository, independent of image destination`, () => {
    for (const repository of [
      "block/buzz",
      "mfethe1/buzz",
      "Other-Owner/Custom.Buzz",
    ]) {
      const cache = resolveCache(job, repository);
      assert.equal(cache, `ghcr.io/${repository.toLowerCase()}`);
      for (const arch of ["amd64", "arm64"]) {
        const context = {
          github: { repository, event_name: "push", ref_protected: true },
          matrix: { arch },
          steps: { cache: { outputs: { repository: cache } } },
          env: { IMAGE_NAME: "ghcr.io/unrelated/release-image" },
        };
        const build = step(job, buildId);
        const expected = `${cache}${suffix}:${arch}`;
        assert.equal(
          expand(build.with["cache-from"], context),
          `type=registry,ref=${expected}`,
        );
        assert.equal(
          expand(build.with["cache-to"], context),
          `type=registry,ref=${expected},mode=max,compression=zstd`,
        );
        if (buildId === "build-release") {
          assert.equal(
            expand(step(job, "build-debug").with["cache-from"], context),
            `type=registry,ref=${expected}`,
          );
        }
      }
    }
  });

  test(`${job.name}: only protected push or rescue dispatch can export cache`, () => {
    const cache = resolveCache(job, "mfethe1/buzz");
    for (const event of [
      "pull_request",
      "pull_request_target",
      "push",
      "workflow_dispatch",
      "schedule",
    ]) {
      for (const protectedRef of [true, false, undefined]) {
        for (const headRepository of ["mfethe1/buzz", "untrusted/fork"]) {
          const context = {
            github: {
              repository: "mfethe1/buzz",
              event_name: event,
              ref_protected: protectedRef,
              event: {
                pull_request: { head: { repo: { full_name: headRepository } } },
              },
            },
            matrix: { arch: "arm64" },
            steps: { cache: { outputs: { repository: cache } } },
            env: { IMAGE_NAME: "ghcr.io/block/buzz" },
          };
          const expectedWrite =
            ["push", "workflow_dispatch"].includes(event) &&
            protectedRef === true;
          assert.equal(
            expand(step(job, buildId).with["cache-to"], context) !== "",
            expectedWrite,
            `${event}, protected=${protectedRef}, head=${headRepository}`,
          );
          const login = job.steps.filter((candidate) =>
            candidate.uses?.startsWith("docker/login-action@"),
          );
          assert.equal(login.length, 1);
          assert.equal(
            evaluate(login[0].if, context),
            ["push", "workflow_dispatch"].includes(event) ? "true" : "false",
          );
          if (event === "pull_request") {
            assert.ok(
              expand(step(job, buildId).with.outputs, context).endsWith(
                "push=false",
              ),
            );
          }
        }
      }
    }
  });
}

test("build failures stay mandatory and source qualification remains independent", () => {
  for (const { job, buildId } of configurations) {
    assert.equal(job["continue-on-error"], undefined);
    const build = step(job, buildId);
    assert.equal(build["continue-on-error"], undefined);
    assert.equal(
      build.if,
      undefined,
      "cache policy must not skip the image build",
    );
    assert.ok(!JSON.stringify(build.with).includes("ignore-error=true"));
    assert.ok(
      job.steps.findIndex((item) => item.id === "cache") <
        job.steps.indexOf(build),
    );
    assert.equal(step(job, "cache").if, undefined);
  }
  assert.deepEqual(workflow.jobs.merge.needs, ["build", "qualify"]);
  assert.equal(workflow.jobs.qualify.if, "github.event_name != 'pull_request'");
  assert.equal(workflow.jobs.qualify.permissions.actions, "read");
  assert.equal(workflow.jobs.build.steps[0].with["persist-credentials"], false);
  assert.equal(
    workflow.jobs["push-gateway-build"].steps[0].with["persist-credentials"],
    false,
  );
});

test("the production check is present in Docker path selection and the CI contract lane", () => {
  assert.ok(
    workflow.on.pull_request.paths.includes(
      ".github/scripts/docker-cache.test.mjs",
    ),
  );
  const ci = parse(
    readFileSync(path.join(root, ".github/workflows/ci.yml"), "utf8"),
  );
  assert.ok(
    ci.jobs.changes.steps.some(
      (item) => item.run === "just docker-cache-check",
    ),
  );
  const justfile = readFileSync(path.join(root, "Justfile"), "utf8");
  assert.match(justfile, /^check:.*\bdocker-cache-check\b/m);
  assert.match(
    justfile,
    /node --test \.github\/scripts\/docker-cache\.test\.mjs/,
  );
});
