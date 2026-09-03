#!/usr/bin/env bun

import { spawnSync } from "node:child_process";

type ArtifactRecord = {
  bytes: number;
  name: string;
  sha256: string;
};

type ReleaseCommit = {
  body: string;
  subject: string;
};

type ReleaseSummary = {
  changes: string[];
  previousTag: string;
  tag: string;
};

type StableVersion = {
  major: number;
  minor: number;
  patch: number;
  tag: string;
};

const repository = "FabienGreard/TouchGrassBar";
const stableTagPattern = /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/u;
const excludedFallbackScopes = new Set(["build", "ci", "dev", "docs", "release", "test"]);

function command(executable: string, argumentsList: string[]) {
  const result = spawnSync(executable, argumentsList, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    throw new Error(result.stderr.trim() || `Release-note command failed: ${executable}.`);
  }
  return result.stdout.trim();
}

function parseStableVersion(tag: string): StableVersion | null {
  const match = stableTagPattern.exec(tag);
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

function previousStableTag(tag: string, tags: readonly string[]) {
  const current = parseStableVersion(tag);
  if (!current) throw new Error(`Release tag is invalid: ${tag}.`);
  const previous = tags
    .map(parseStableVersion)
    .filter(
      (candidate): candidate is StableVersion =>
        candidate !== null && compareVersions(candidate, current) < 0,
    )
    .sort(compareVersions)
    .at(-1);
  if (!previous) throw new Error(`Previous stable release tag is absent for ${tag}.`);
  return previous.tag;
}

function sentence(value: string) {
  const withoutPullRequest = value.trim().replace(/\s+\(#[0-9]+\)$/u, "");
  if (withoutPullRequest.length === 0) throw new Error("Release-note text is empty.");
  const capitalized = `${withoutPullRequest[0]?.toUpperCase()}${withoutPullRequest.slice(1)}`;
  return /[.!?]$/u.test(capitalized) ? capitalized : `${capitalized}.`;
}

function releaseChangesFromCommits(commits: readonly ReleaseCommit[]) {
  const replacements = commits.filter((commit) =>
    /^Release-note-mode:[ \t]*replace[ \t]*$/imu.test(commit.body),
  );
  if (replacements.length > 1) {
    throw new Error("Release notes have more than one replacement summary.");
  }
  const replacement = replacements[0];
  const selectedCommits = replacement
    ? commits.slice(commits.indexOf(replacement))
    : commits;
  const changes: string[] = [];
  const seen = new Set<string>();
  for (const commit of selectedCommits) {
    const trailers = [
      ...commit.body.matchAll(/^Release-note:[ \t]*(\S(?:.*\S)?)[ \t]*$/gimu),
    ].map((match) => match[1]!);
    const reviewedTrailers = trailers.filter((trailer) => trailer.toLowerCase() !== "none");
    if (commit === replacement && reviewedTrailers.length === 0) {
      throw new Error("The replacement release summary has no user-facing note.");
    }
    const candidates =
      trailers.length > 0
        ? reviewedTrailers
        : (() => {
            const conventional = /^(feat|fix|perf)(?:\(([^)]+)\))?!?:\s+(.+)$/u.exec(
              commit.subject,
            );
            if (!conventional) return [];
            const scope = conventional[2]?.toLowerCase();
            if (scope && excludedFallbackScopes.has(scope)) return [];
            return [conventional[3]!];
          })();
    for (const candidate of candidates) {
      const change = sentence(candidate);
      const key = change.toLowerCase();
      if (seen.has(key)) continue;
      seen.add(key);
      changes.push(change);
    }
  }
  return changes;
}

function releaseHistoryArguments(previousTag: string, target: string) {
  return [
    "log",
    "--cherry-pick",
    "--right-only",
    "--first-parent",
    "--reverse",
    "--format=%s%x00%b%x1e",
    `${previousTag}...${target}`,
  ];
}

function releaseCommits(previousTag: string, target: string) {
  command("git", ["rev-parse", "--verify", `${previousTag}^{commit}`]);
  command("git", ["rev-parse", "--verify", `${target}^{commit}`]);
  const output = command("git", releaseHistoryArguments(previousTag, target));
  if (output.length === 0) return [];
  return output.split("\x1e").flatMap((rawRecord) => {
    const record = rawRecord.trim();
    if (record.length === 0) return [];
    const separator = record.indexOf("\x00");
    if (separator < 1) throw new Error("Release commit history is invalid.");
    return [
      {
        body: record.slice(separator + 1).trim(),
        subject: record.slice(0, separator).trim(),
      },
    ];
  });
}

function createReleaseSummary(previousTag: string, tag: string, target = tag): ReleaseSummary {
  if (!stableTagPattern.test(previousTag) || !stableTagPattern.test(tag)) {
    throw new Error("Release-note tag range is invalid.");
  }
  const changes = releaseChangesFromCommits(releaseCommits(previousTag, target));
  if (changes.length === 0) {
    throw new Error(`Release ${tag} has no user-facing release note.`);
  }
  return { changes, previousTag, tag };
}

function createReleaseSummaryForTag(tag: string) {
  const tags = command("git", ["tag", "--list", "v*"]).split(/\r?\n/u);
  return createReleaseSummary(previousStableTag(tag, tags), tag);
}

function assetOrder(name: string) {
  if (name.endsWith(".dmg")) return 0;
  if (name.endsWith(".app.tar.gz")) return 1;
  if (name.endsWith(".app.tar.gz.sig")) return 2;
  if (name === "latest.json") return 3;
  if (name === "SHA256SUMS") return 4;
  if (name.startsWith("database-compatibility-")) return 5;
  if (name.startsWith("release-trust-")) return 6;
  return 7;
}

function createReleaseNotes(summary: ReleaseSummary, records: readonly ArtifactRecord[]) {
  const version = summary.tag.slice(1);
  const expectedDmg = `TouchGrassBar_${version}_aarch64.dmg`;
  if (!records.some((record) => record.name === expectedDmg)) {
    throw new Error(`Release notes are missing the expected DMG for ${summary.tag}.`);
  }
  const releaseFiles = [...records]
    .sort(
      (left, right) =>
        assetOrder(left.name) - assetOrder(right.name) || left.name.localeCompare(right.name),
    )
    .map((record) => {
      if (!/^[0-9a-f]{64}$/u.test(record.sha256) || !Number.isSafeInteger(record.bytes)) {
        throw new Error(`Release-note artifact is invalid: ${record.name}.`);
      }
      return `- \`${record.name}\` — ${record.bytes.toLocaleString("en-US")} bytes — SHA-256 \`${record.sha256}\``;
    })
    .join("\n");
  const changes = summary.changes.map((change) => `- ${change}`).join("\n");
  const compareUrl = `https://github.com/${repository}/compare/${summary.previousTag}...${summary.tag}`;
  return `## What changed

${changes}

## Download

Download \`${expectedDmg}\` from the Assets section for Apple silicon Macs.

<details>
<summary>Technical verification</summary>

- Developer ID signature, hardened runtime, and timestamp: PASS
- App and DMG notarization and stapling: PASS
- App and DMG Gatekeeper assessment: PASS
- Tauri updater signature: PASS
- Apple silicon artifact binding: PASS

### Release files

${releaseFiles}

</details>

**Full changelog:** [${summary.previousTag}...${summary.tag}](${compareUrl})
`;
}

function updaterReleaseNotes(changes: readonly string[]) {
  const notes = changes.join(" ").trim();
  if (notes.length === 0) throw new Error("Updater release notes are empty.");
  return notes;
}

export {
  createReleaseNotes,
  createReleaseSummary,
  createReleaseSummaryForTag,
  previousStableTag,
  releaseChangesFromCommits,
  releaseHistoryArguments,
  updaterReleaseNotes,
};
export type { ArtifactRecord, ReleaseCommit, ReleaseSummary };
