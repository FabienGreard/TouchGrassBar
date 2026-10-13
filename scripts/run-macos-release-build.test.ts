import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { setTimeout as wait } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { expect, test } from "vitest";

import { runMacosReleaseBuild } from "./run-macos-release-build";
import {
  parseStapleContext,
  runReleaseXcrun,
  stapleContextEnvironmentName,
} from "./run-release-xcrun";

const environment = {
  PATH: "/usr/bin:/bin",
  RELEASE_COMMIT: "a".repeat(40),
  RELEASE_TAG: "v0.0.61",
};
const succeeds = { run: () => ({ status: 0 }), wait: async () => {} };

test("the build cannot complete when Tauri ignores a failed app staple", async () => {
  await expect(
    runMacosReleaseBuild({
      environment,
      run: async (_command, options) => {
        const context = parseStapleContext(options.env[stapleContextEnvironmentName]!);
        expect(
          await runReleaseXcrun(["stapler", "staple", context.appPath], context, {
            run: () => ({ status: 68 }),
            wait: async () => {},
          }),
        ).toBe(1);
        return 0; // Pinned Tauri ignores the stapler child exit.
      },
    }),
  ).rejects.toThrow("The macOS release app staple proof failed.");
});

test("the exact build completes only after a fresh app staple and validation proof", async () => {
  let temporary = "";
  const signingValue = "synthetic-protected-value";
  await runMacosReleaseBuild({
    environment: { ...environment, TAURI_SIGNING_PRIVATE_KEY: signingValue },
    run: async (command, options) => {
      expect(command).toEqual([
        "bun",
        "run",
        "--cwd",
        "apps/desktop",
        "tauri",
        "build",
        "--bundles",
        "app",
        "dmg",
        "--config",
        "src-tauri/.dev-instance/prod-tauri.conf.json",
      ]);
      const context = parseStapleContext(options.env[stapleContextEnvironmentName]!);
      temporary = dirname(context.journalPath);
      expect(options.env.PATH?.startsWith(temporary + ":")).toBe(true);
      expect(statSync(temporary).mode & 0o777).toBe(0o700);
      const wrapper = join(temporary, "xcrun");
      expect(statSync(wrapper).mode & 0o777).toBe(0o700);
      expect(readFileSync(wrapper, "utf8")).not.toContain(signingValue);
      expect(options.env.TAURI_SIGNING_PRIVATE_KEY).toBe(signingValue);
      await runReleaseXcrun(["stapler", "staple", context.appPath], context, succeeds);
      expect(readFileSync(context.journalPath, "utf8")).not.toContain(signingValue);
      return 0;
    },
  });
  expect(existsSync(temporary)).toBe(false);
});

test("a successful child with no app staple proof cannot complete the release build", async () => {
  await expect(runMacosReleaseBuild({ environment, run: async () => 0 })).rejects.toThrow(
    "The macOS release app staple proof failed.",
  );
});

test.each(["attemptId", "commit", "tag", "appPath"] as const)(
  "an old or mismatched %s proof cannot authorize a build",
  async (field) => {
    await expect(
      runMacosReleaseBuild({
        environment,
        run: async (_command, options) => {
          const context = parseStapleContext(options.env[stapleContextEnvironmentName]!);
          const old = {
            ...context,
            [field]: field === "appPath" ? context.appPath + "/other.app" : "old-value",
          };
          await runReleaseXcrun(["stapler", "staple", old.appPath], old, succeeds);
          return 0;
        },
      }),
    ).rejects.toThrow("The macOS release app staple proof failed.");
  },
);

test("a later PASS cannot erase an earlier failed staple in the same build", async () => {
  await expect(
    runMacosReleaseBuild({
      environment,
      run: async (_command, options) => {
        const context = parseStapleContext(options.env[stapleContextEnvironmentName]!);
        await runReleaseXcrun(["stapler", "staple", context.appPath], context, {
          run: () => ({ status: 68 }),
          wait: async () => {},
        });
        await runReleaseXcrun(["stapler", "staple", context.appPath], context, succeeds);
        return 0;
      },
    }),
  ).rejects.toThrow("The macOS release app staple proof failed.");
});

test("temporary wrapper and proof are removed when the build throws", async () => {
  let temporary = "";
  await expect(
    runMacosReleaseBuild({
      environment,
      run: async (_command, options) => {
        temporary = dirname(
          parseStapleContext(options.env[stapleContextEnvironmentName]!).journalPath,
        );
        throw new Error("Synthetic build failure");
      },
    }),
  ).rejects.toThrow();
  expect(existsSync(temporary)).toBe(false);
});

test("cancellation stops a descendant even when its build leader exits first", async () => {
  const directory = mkdtempSync(join(tmpdir(), "tgb-release-cancel-"));
  const bin = join(directory, "bin");
  mkdirSync(bin);
  const childPidPath = join(directory, "child-pid");
  const childSource = `process.on("SIGTERM", () => {}); require("node:fs").writeFileSync(process.env.TGB_TEST_CHILD_PID, String(process.pid)); setInterval(() => {}, 1000);`;
  writeFileSync(
    join(bin, "bun"),
    `#!${process.execPath}\nconst {spawn}=require("node:child_process"); process.on("SIGTERM",()=>process.exit(0)); spawn(process.execPath,["-e",${JSON.stringify(childSource)}],{stdio:"ignore"});\n`,
    { mode: 0o700 },
  );
  const source = fileURLToPath(new URL("./run-macos-release-build.ts", import.meta.url));
  const driverPath = join(directory, "driver.ts");
  writeFileSync(
    driverPath,
    `import {runMacosReleaseBuild} from ${JSON.stringify(source)}; try { await runMacosReleaseBuild({root:process.env.TGB_TEST_ROOT, environment:{...process.env, PATH:process.env.TGB_TEST_BUILD_PATH}}); process.exitCode=0; } catch { process.exitCode=1; }`,
  );
  const driver = spawn("bun", ["--no-env-file", driverPath], {
    stdio: "ignore",
    env: {
      ...process.env,
      ...environment,
      PATH: process.env.PATH,
      TGB_TEST_ROOT: directory,
      TGB_TEST_BUILD_PATH: bin + ":" + process.env.PATH,
      TGB_TEST_CHILD_PID: childPidPath,
    },
  });
  const ended = new Promise<number | null>((done, reject) => {
    driver.on("close", done);
    driver.on("error", reject);
  });
  let childPid: number | undefined;
  try {
    const deadline = Date.now() + 4_000;
    while (!existsSync(childPidPath) && Date.now() < deadline) await wait(20);
    expect(existsSync(childPidPath)).toBe(true);
    childPid = Number(readFileSync(childPidPath, "utf8"));
    driver.kill("SIGTERM");
    expect(await ended).toBe(1);
    await wait(200);
    const state = spawnSync("/bin/ps", ["-p", String(childPid), "-o", "stat="], {
      encoding: "utf8",
    }).stdout.trim();
    expect(state !== "" && !state.startsWith("Z")).toBe(false);
  } finally {
    if (driver.exitCode === null) driver.kill("SIGKILL");
    if (childPid !== undefined) {
      try {
        process.kill(childPid, "SIGKILL");
      } catch {
        /* Already stopped. */
      }
    }
    rmSync(directory, { recursive: true, force: true });
  }
}, 8_000);
