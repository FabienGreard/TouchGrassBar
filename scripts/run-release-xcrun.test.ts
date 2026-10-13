import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, expect, test } from "vitest";

import { runReleaseXcrun, runXcrunCommand, type StapleContext } from "./run-release-xcrun";

const directories: string[] = [];
afterEach(() => {
  for (const directory of directories.splice(0))
    rmSync(directory, { recursive: true, force: true });
});
function context(): StapleContext {
  const directory = mkdtempSync(join(tmpdir(), "tgb-staple-test-"));
  directories.push(directory);
  return {
    appPath: join(directory, "TouchGrassBar.app"),
    attemptId: "fresh-attempt",
    commit: "a".repeat(40),
    journalPath: join(directory, "journal"),
    tag: "v0.0.61",
  };
}

test("a failed staple cannot produce a PASS even when Tauri ignores its exit", async () => {
  const selected = context();
  const calls: string[][] = [];
  const outcome = await runReleaseXcrun(["stapler", "staple", selected.appPath], selected, {
    run: (args, quiet, timeoutMs) => {
      calls.push([...args]);
      expect(quiet).toBe(true);
      expect(timeoutMs).toBe(30_000);
      return { status: 68 };
    },
    wait: async () => {},
  });
  expect(outcome).toBe(1);
  expect(calls).toHaveLength(3);
  const journal = readFileSync(selected.journalPath, "utf8");
  expect(journal).toContain('"status":"failed"');
  expect(journal).not.toContain('"status":"passed"');
});

test("a PASS requires a successful validate after staple and allows a bounded retry", async () => {
  const selected = context();
  const calls: string[] = [];
  let validations = 0;
  const result = await runReleaseXcrun(["stapler", "staple", "-v", selected.appPath], selected, {
    run: (args) => {
      calls.push(args[1]!);
      return { status: args[1] === "validate" && validations++ === 0 ? 68 : 0 };
    },
    wait: async () => {},
  });
  expect(result).toBe(0);
  expect(calls).toEqual(["staple", "validate", "staple", "validate"]);
  expect(readFileSync(selected.journalPath, "utf8")).toContain('"status":"passed"');
  expect(statSync(selected.journalPath).mode & 0o777).toBe(0o700);
});

test("a timeout cannot authorize PASS even if a runner reports exit zero", async () => {
  const selected = context();
  let calls = 0;
  expect(
    await runReleaseXcrun(["stapler", "staple", selected.appPath], selected, {
      run: () => {
        calls += 1;
        return { status: 0, timedOut: true };
      },
      wait: async () => {},
    }),
  ).toBe(1);
  expect(calls).toBe(3);
  expect(readFileSync(selected.journalPath, "utf8")).not.toContain('"status":"passed"');
});

test("successful staple calls cannot hide validation failures after the retry cap", async () => {
  const selected = context();
  let calls = 0;
  expect(
    await runReleaseXcrun(["stapler", "staple", selected.appPath], selected, {
      run: (args) => {
        calls += 1;
        return { status: args[1] === "staple" ? 0 : 65 };
      },
      wait: async () => {},
    }),
  ).toBe(1);
  expect(calls).toBe(6);
  expect(readFileSync(selected.journalPath, "utf8")).not.toContain('"status":"passed"');
});

test("the child command timeout stops a blocked tool without emitting its output", () => {
  const started = Date.now();
  const result = runXcrunCommand(process.execPath, ["-e", "setTimeout(() => {}, 5000)"], true, 30);
  expect(result.timedOut).toBe(true);
  expect(result.status).not.toBe(0);
  expect(Date.now() - started).toBeLessThan(1_000);
});

test.each([
  ["--sdk", "macosx", "--show-sdk-path"],
  ["stapler", "staple", "/different.app"],
])("unrelated xcrun operations pass through without a staple proof: %j", async (...args) => {
  const selected = context();
  const calls: Array<{ args: readonly string[]; quiet: boolean; timeoutMs: number | undefined }> =
    [];
  expect(
    await runReleaseXcrun(args, selected, {
      run: (forwarded, quiet, timeoutMs) => {
        calls.push({ args: forwarded, quiet, timeoutMs });
        return { status: 7 };
      },
      wait: async () => {
        throw new Error("Unexpected retry");
      },
    }),
  ).toBe(7);
  expect(calls).toEqual([{ args, quiet: false, timeoutMs: undefined }]);
  expect(existsSync(selected.journalPath)).toBe(false);
});
