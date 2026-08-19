import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, test } from "vitest";

import {
  releaseEnvironmentVariables,
  releaseSecrets,
  stableUpdaterEndpoint,
} from "./release-contract";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (...parts: string[]) => readFileSync(resolve(root, ...parts), "utf8");

describe("release governance", () => {
  test("uses readable Action versions and keeps the draft gate in order", () => {
    const ci = read(".github", "workflows", "ci.yml");
    const release = read(".github", "workflows", "release.yml");
    const uses = [ci, release].flatMap((source) => [
      ...source.matchAll(/^\s*-?\s*uses:\s*([^@\s]+)@([^\s#]+)/gmu),
    ]);

    expect(uses.length).toBeGreaterThan(0);
    expect(
      uses.every((match) =>
        /^(?:stable|v[0-9]+(?:\.[0-9]+){0,2})$/.test(match[2] ?? ""),
      ),
    ).toBe(true);
    expect(release.indexOf("validate-source")).toBeLessThan(
      release.indexOf("name: macos-release"),
    );
    expect(release).toContain("$RUNNER_TEMP/touchgrass-release-private");
    expect(release).toContain("if: ${{ always() }}");
    expect(release).toContain("--draft");
    expect(release).not.toContain("environment: public-release");
    expect([ci, release].join("\n")).not.toContain("persist-credentials: true");
  });

  test("keeps the exact policy and release-only updater settings", () => {
    const governance = JSON.parse(
      read(".github", "release-governance.json"),
    ) as Record<string, any>;
    const config = JSON.parse(
      read("apps", "desktop", "src-tauri", "tauri.conf.json"),
    ) as Record<string, any>;
    const productionScript = read("scripts", "run-desktop-prod.ts");
    const releaseWorkflow = read(".github", "workflows", "release.yml");

    expect(governance.environments["macos-release"]).toMatchObject({
      deployment_tag_patterns: ["v*"],
      required_reviewers: ["FabienGreard"],
      secrets: releaseSecrets,
      variables: releaseEnvironmentVariables,
    });
    expect(governance.environments["public-release"].required_reviewers).toEqual([
      "FabienGreard",
    ]);
    expect(governance.actions).toMatchObject({
      allowed_actions: "selected",
      sha_pinning_required: false,
    });
    expect(governance.tag_ruleset.rules).toEqual(["update", "deletion"]);
    expect(governance.immutable_releases).toBe(true);
    for (const name of [...releaseSecrets, ...releaseEnvironmentVariables]) {
      expect(releaseWorkflow).toContain(`RELEASE_HAS_${name}`);
    }
    expect(config.plugins.updater.endpoints).toEqual([stableUpdaterEndpoint]);
    expect(config.bundle?.createUpdaterArtifacts).toBeUndefined();
    expect(productionScript).toContain("createUpdaterArtifacts: true");
  });
});
