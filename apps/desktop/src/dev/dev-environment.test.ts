import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, test } from "vitest";

import { convexCommandEnvironment } from "../../../../scripts/convex-command-environment";
import { coordinatedProcessExitCode } from "../../../../scripts/coordinated-process-exit";

type TurboConfiguration = {
  tasks?: {
    dev?: {
      passThroughEnv?: string[];
    };
  };
};

type RootPackage = {
  scripts?: Record<string, string>;
};

type WorkspacePackage = {
  scripts?: Record<string, string>;
};

describe("desktop development environment", () => {
  test("treats only coordinated process signals as a clean shutdown", () => {
    expect(coordinatedProcessExitCode(143, "SIGTERM")).toBe(0);
    expect(coordinatedProcessExitCode(130, "SIGINT")).toBe(0);
    expect(coordinatedProcessExitCode(143, null)).toBe(143);
    expect(coordinatedProcessExitCode(1, "SIGTERM")).toBe(1);
  });

  test("uses anonymous agent mode only for the selected local backend", () => {
    const selectedLocal = {
      CONVEX_DEPLOYMENT: "anonymous:anonymous-agent",
    };

    expect(
      convexCommandEnvironment(["dev"], {}, selectedLocal).CONVEX_AGENT_MODE,
    ).toBe("anonymous");
    expect(
      convexCommandEnvironment(["login"], {}, selectedLocal)
        .CONVEX_AGENT_MODE,
    ).toBeUndefined();
    expect(
      convexCommandEnvironment(["deploy"], {}, selectedLocal)
        .CONVEX_AGENT_MODE,
    ).toBeUndefined();
    expect(
      convexCommandEnvironment(
        ["dev"],
        { CONVEX_AGENT_MODE: "anonymous" },
        { CONVEX_DEPLOYMENT: "dev:shared-development" },
      ).CONVEX_AGENT_MODE,
    ).toBeUndefined();
  });

  test("passes the selected Convex service URLs to native builds", () => {
    const configuration = JSON.parse(
      readFileSync(
        resolve(dirname(fileURLToPath(import.meta.url)), "../../../../turbo.json"),
        "utf8",
      ),
    ) as TurboConfiguration;

    expect(configuration.tasks?.dev?.passThroughEnv).toEqual(
      expect.arrayContaining([
        "CONVEX_SITE_URL",
        "CONVEX_URL",
      ]),
    );
  });

  test("exposes one cwd-independent development command surface", () => {
    const root = resolve(
      dirname(fileURLToPath(import.meta.url)),
      "../../../../",
    );
    const packageManifest = JSON.parse(
      readFileSync(resolve(root, "package.json"), "utf8"),
    ) as RootPackage;
    const desktopManifest = JSON.parse(
      readFileSync(resolve(root, "apps/desktop/package.json"), "utf8"),
    ) as WorkspacePackage;
    const backendManifest = JSON.parse(
      readFileSync(resolve(root, "packages/backend/package.json"), "utf8"),
    ) as WorkspacePackage;

    expect(packageManifest.scripts?.dev).toContain("scripts/run-dev.ts all");
    expect(packageManifest.scripts?.["dev:desktop"]).toContain(
      "scripts/run-dev.ts desktop",
    );
    expect(packageManifest.scripts?.["convex:login"]).toContain(
      "packages/backend convex login",
    );
    expect(packageManifest.scripts?.["convex:dev"]).toContain(
      "packages/backend convex dev",
    );
    expect(packageManifest.scripts?.["convex:prod"]).toContain(
      "packages/backend convex deploy",
    );
    expect(packageManifest.scripts?.reset).toBe("bun scripts/reset.ts");
    expect(packageManifest.scripts?.["reset:bundle"]).toBe(
      "bun scripts/reset.ts --bundle",
    );
    expect(packageManifest.scripts?.["reset:release"]).toBe(
      "bun scripts/reset.ts --release",
    );
    expect(packageManifest.scripts?.["reset:prod"]).toBeUndefined();
    expect(packageManifest.scripts?.["worktree:setup"]).toBeUndefined();
    expect(packageManifest.scripts?.prod).toBeUndefined();
    expect(desktopManifest.scripts?.dev).toBe(
      "bun run --cwd ../.. dev:desktop",
    );
    expect(desktopManifest.scripts?.bundle).toBe(
      "bun run --cwd ../.. desktop:bundle",
    );
    expect(backendManifest.scripts?.convex).toBe(
      "bun ../../scripts/run-convex.ts",
    );
  });
});
