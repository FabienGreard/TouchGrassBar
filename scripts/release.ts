#!/usr/bin/env bun

import { spawnSync } from "node:child_process";

import {
  publishedStableReleaseTags,
  releaseRepository,
  verifyDatabaseFixtureCandidate,
} from "./release-contract";

type ReleaseLevel = "major" | "minor" | "patch";
type StableVersion = {
  major: number;
  minor: number;
  patch: number;
  tag: string;
};

const usage = "Use: bun run release <patch|minor|major> [--execute].";

function command(executable: string, argumentsList: string[]) {
  const result = spawnSync(executable, argumentsList, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    const detail = result.stderr.trim();
    throw new Error(detail || `Release command failed: ${executable}.`);
  }
  return result.stdout.trim();
}

function parseStableVersion(tag: string): StableVersion | null {
  const match = /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.exec(tag);
  if (!match) return null;
  const [major, minor, patch] = match.slice(1).map(Number);
  if (
    !Number.isSafeInteger(major) ||
    !Number.isSafeInteger(minor) ||
    !Number.isSafeInteger(patch)
  ) {
    return null;
  }
  return { major, minor, patch, tag };
}

function compareVersions(left: StableVersion, right: StableVersion) {
  return left.major - right.major || left.minor - right.minor || left.patch - right.patch;
}

function latestStableTag(tags: string[]) {
  const versions = tags
    .map(parseStableVersion)
    .filter((version): version is StableVersion => version !== null)
    .sort(compareVersions);
  const latest = versions.at(-1);
  if (!latest) throw new Error("The repository has no stable release tag.");
  return latest;
}

function bumpStableVersion(current: StableVersion, level: ReleaseLevel) {
  if (level === "major") return `v${current.major + 1}.0.0`;
  if (level === "minor") return `v${current.major}.${current.minor + 1}.0`;
  return `v${current.major}.${current.minor}.${current.patch + 1}`;
}

function parseReleaseArguments(argumentsList: string[]) {
  const level = argumentsList[0] as ReleaseLevel | undefined;
  const flags = argumentsList.slice(1);
  const supportedFlags = new Set(["--execute"]);
  if (
    !level ||
    !new Set<ReleaseLevel>(["major", "minor", "patch"]).has(level) ||
    flags.some((flag) => !supportedFlags.has(flag)) ||
    new Set(flags).size !== flags.length
  ) {
    throw new Error(usage);
  }
  const execute = flags.includes("--execute");
  return { execute, level };
}

function remoteStableTags() {
  const output = command("git", ["ls-remote", "--tags", "--refs", "origin", "refs/tags/v*"]);
  return output.split(/\r?\n/u).flatMap((line) => {
    const match = /\srefs\/tags\/(v[^\s]+)$/u.exec(line);
    return match?.[1] ? [match[1]] : [];
  });
}

function exactMainCommit() {
  if (command("git", ["status", "--porcelain"]) !== "") {
    throw new Error("Release requires a clean worktree.");
  }
  if (command("git", ["branch", "--show-current"]) !== "main") {
    throw new Error("Release requires the main branch.");
  }
  command("git", ["fetch", "--no-tags", "origin", "+refs/heads/main:refs/remotes/origin/main"]);
  const head = command("git", ["rev-parse", "HEAD"]);
  const remoteMain = command("git", ["rev-parse", "origin/main"]);
  if (head !== remoteMain) {
    throw new Error("Release requires exact remote main.");
  }
  return head;
}

function exactMainCiRun(commit: string) {
  const output = command("gh", [
    "run",
    "list",
    "--repo",
    releaseRepository,
    "--workflow",
    ".github/workflows/ci.yml",
    "--branch",
    "main",
    "--event",
    "push",
    "--commit",
    commit,
    "--status",
    "success",
    "--limit",
    "1",
    "--json",
    "conclusion,databaseId,headSha",
  ]);
  const run = (
    JSON.parse(output) as Array<{
      conclusion?: unknown;
      databaseId?: unknown;
      headSha?: unknown;
    }>
  )[0];
  if (
    run?.conclusion !== "success" ||
    run.headSha !== commit ||
    typeof run.databaseId !== "number"
  ) {
    throw new Error("Release requires successful exact-head main CI.");
  }
  return String(run.databaseId);
}

function verifyGovernance() {
  command("bun", ["run", "release:governance", "--verify"]);
}

function createAndPushTag(tag: string) {
  if (command("git", ["tag", "--list", tag]) !== "") {
    throw new Error(`Local release tag already exists: ${tag}.`);
  }
  command("git", ["tag", "-a", tag, "-m", `TouchGrassBar ${tag}`]);
  try {
    command("git", ["push", "origin", `refs/tags/${tag}`]);
  } catch (error) {
    command("git", ["tag", "--delete", tag]);
    throw error;
  }
}

function release(argumentsList: string[]) {
  const options = parseReleaseArguments(argumentsList);
  const commit = exactMainCommit();
  const ciRun = exactMainCiRun(commit);
  verifyGovernance();
  const remoteTags = remoteStableTags();
  const current = latestStableTag(remoteTags);
  const next = bumpStableVersion(current, options.level);
  verifyDatabaseFixtureCandidate(next, publishedStableReleaseTags(releaseRepository));
  if (remoteTags.includes(next)) {
    throw new Error(`Remote release tag already exists: ${next}.`);
  }

  console.log(`Release plan: ${options.level} ${current.tag} -> ${next}`);
  console.log(`Commit: ${commit}`);
  console.log(`Main CI run: ${ciRun}`);
  console.log("Release governance: PASS");

  if (!options.execute) {
    console.log("Action: preview only");
    console.log(`Execute: bun run release ${options.level} --execute`);
    return;
  }

  createAndPushTag(next);
  console.log(`Release tag pushed: ${next}`);
  console.log(
    `Release workflow: https://github.com/${releaseRepository}/actions/workflows/release.yml`,
  );
}

if (import.meta.main) release(process.argv.slice(2));

export { bumpStableVersion, latestStableTag, parseReleaseArguments, parseStableVersion };
