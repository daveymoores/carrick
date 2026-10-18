/**
 * `carrick doctor` on a workspace where Carrick builds the index (carrick#2219).
 *
 * There Carrick rescans every push itself, so a repo needs no CI workflow and
 * doctor stops asking for one. The lane comes from one `resolve-repos` read,
 * injected here so no test reaches the network.
 */
import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { checkWorkflow, configuredRepos, findingCount, readLane, type LaneDeps, type Line } from "../src/init/doctor.ts";
import { renderTemplate, TEMPLATE_PATHS } from "../src/templates.ts";

const workflowPath = TEMPLATE_PATHS.workflow;

function repoWith(files: Record<string, string>): string {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "carrick-doctor-lane-")));
  const all = { "carrick.json": JSON.stringify({ serviceName: "api" }), ...files };
  for (const [relative, body] of Object.entries(all)) {
    const target = path.join(root, relative);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, body);
  }
  return root;
}

function texts(lines: Line[]): string {
  return lines.map((line) => `${line.level}: ${line.text}`).join("\n");
}

test("cloud lane: no workflow is no line at all", () => {
  const root = repoWith({});
  const lines = checkWorkflow(configuredRepos(root), "cloud");
  assert.deepEqual(lines, []);
  fs.rmSync(root, { recursive: true, force: true });
});

test("cloud lane: a workflow present gets one note, no drift and no pin finding", () => {
  // Pinned and missing template lines: both findings on the laptop lane.
  const body = renderTemplate("workflow", { ACTION_REF: "carrick-tools/carrick@v1.4.2" })
    .split("\n")
    .filter((line) => !line.includes("repository_dispatch"))
    .join("\n");
  const root = repoWith({ [workflowPath]: body });
  assert.ok(findingCount(checkWorkflow(configuredRepos(root), "laptop")) >= 1, "the laptop lane flags it");
  const lines = checkWorkflow(configuredRepos(root), "cloud");
  assert.equal(lines.length, 1, texts(lines));
  assert.equal(lines[0]!.level, "say");
  assert.equal(findingCount(lines), 0);
  assert.ok(
    lines[0]!.text.endsWith(
      ": Carrick now scans this repo in its own cloud runners on every push, so .github/workflows/carrick.yml is no longer needed and only uses CI minutes. You can delete it.",
    ),
    lines[0]!.text,
  );
  fs.rmSync(root, { recursive: true, force: true });
});

test("laptop lane: today's warning, by default and when named", () => {
  const root = repoWith({});
  for (const lines of [checkWorkflow(configuredRepos(root)), checkWorkflow(configuredRepos(root), "laptop")]) {
    assert.equal(findingCount(lines), 1, texts(lines));
    assert.match(lines[0]!.text, /no \.github\/workflows\/carrick\.yml, so pushes to the default branch do not re-index it/);
  }
  fs.rmSync(root, { recursive: true, force: true });
});

function deps(over: Partial<LaneDeps> & { builder?: unknown } = {}): LaneDeps & { calls: string[][] } {
  const calls: string[][] = [];
  return {
    calls,
    credential: () => ({ token: "t" }),
    repoName: () => "acme/api",
    resolve: (async (_token: string, names: string[]) => {
      calls.push(names);
      return { workspace: { index_builder: over.builder } } as never;
    }) as LaneDeps["resolve"],
    ...over,
  };
}

test("the lane is cloud only when the read says cloud", async () => {
  const root = repoWith({});
  const repos = configuredRepos(root);
  const cloud = deps({ builder: "cloud" });
  assert.equal(await readLane(repos, cloud), "cloud");
  assert.deepEqual(cloud.calls, [["acme/api"]]);
  for (const builder of ["laptop", undefined, "something-new"]) {
    assert.equal(await readLane(repos, deps({ builder })), "laptop", String(builder));
  }
  fs.rmSync(root, { recursive: true, force: true });
});

test("no credential, a failed read or no named repo is the laptop lane and never throws", async () => {
  const root = repoWith({});
  const repos = configuredRepos(root);
  const anonymous = deps({ builder: "cloud", credential: () => null });
  assert.equal(await readLane(repos, anonymous), "laptop");
  assert.deepEqual(anonymous.calls, [], "no credential, no request");
  const down = deps({
    resolve: (async () => {
      throw new Error("down");
    }) as LaneDeps["resolve"],
  });
  assert.equal(await readLane(repos, down), "laptop");
  const broken = deps({
    credential: () => {
      throw new Error("bad credential");
    },
  });
  assert.equal(await readLane(repos, broken), "laptop");
  const unnamed = deps({ builder: "cloud", repoName: () => null });
  assert.equal(await readLane(repos, unnamed), "laptop");
  assert.deepEqual(unnamed.calls, [], "a repo with no GitHub remote is named in no request");
  fs.rmSync(root, { recursive: true, force: true });
});
