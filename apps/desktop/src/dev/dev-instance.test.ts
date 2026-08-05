import { describe, expect, test } from "vitest";

import { resolveDevInstance } from "@/dev/dev-instance";

describe("development instance identity", () => {
  test("derives one stable identity from an issue branch and worktree", () => {
    const input = {
      branch: "agent/issue-47-identify-dev-instances",
      worktreeSeed: "worktree-alpha",
    };

    const first = resolveDevInstance(input);
    const second = resolveDevInstance(input);

    expect(first).toEqual(second);
    expect(first.label).toBe("#47 Identify dev instances");
    expect(first.tag).toBe("#47");
    expect(first.identifier).toMatch(/^app\.touchgrass\.bar\.dev\.w[a-z0-9]+$/);
    expect(first.productName).toContain("TouchGrassBar Dev #47");
    expect(first.port).toBeGreaterThanOrEqual(15_000);
    expect(first.port).toBeLessThan(16_000);
  });

  test("accepts bounded label, accent, and port overrides", () => {
    const instance = resolveDevInstance({
      accent: "violet",
      branch: "feature/cache-refresh",
      label: "  Cache   refresh  ",
      port: 15_222,
      worktreeSeed: "worktree-beta",
    });

    expect(instance.label).toBe("Cache refresh");
    expect(instance.accent).toBe("violet");
    expect(instance.port).toBe(15_222);
  });

  test("separates native identity and default ports across worktrees", () => {
    const first = resolveDevInstance({
      branch: "feature/parallel-preview",
      worktreeSeed: "worktree-one",
    });
    const second = resolveDevInstance({
      branch: "feature/parallel-preview",
      worktreeSeed: "worktree-two",
    });

    expect(first.key).not.toBe(second.key);
    expect(first.identifier).not.toBe(second.identifier);
    expect(first.productName).not.toBe(second.productName);
    expect(first.port).not.toBe(second.port);
  });
});
