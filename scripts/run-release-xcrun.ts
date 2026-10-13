import { spawnSync } from "node:child_process";
import { closeSync, constants, openSync, writeSync } from "node:fs";
import { isAbsolute, resolve } from "node:path";
import { setTimeout } from "node:timers/promises";

export type StapleContext = {
  appPath: string;
  attemptId: string;
  commit: string;
  journalPath: string;
  tag: string;
};
export type ToolResult = { status: number | null; timedOut?: boolean };
export type XcrunPorts = {
  run: (args: readonly string[], quiet: boolean, timeoutMs?: number) => ToolResult;
  wait: (milliseconds: number) => Promise<void>;
};
export const stapleContextEnvironmentName = "TOUCHGRASS_RELEASE_STAPLE_CONTEXT";
const commandTimeoutMs = 30_000;
const retryDelaysMs = [20_000, 40_000] as const;

export function parseStapleContext(value: string): StapleContext {
  const context = JSON.parse(value) as StapleContext;
  if (
    !context ||
    !isAbsolute(context.appPath) ||
    !context.appPath.endsWith(".app") ||
    !isAbsolute(context.journalPath) ||
    !/^[a-zA-Z0-9-]{1,96}$/.test(context.attemptId) ||
    !/^[a-f0-9]{40}$/.test(context.commit) ||
    !/^v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/.test(context.tag)
  ) {
    throw new Error("The release staple context is invalid.");
  }
  return {
    appPath: resolve(context.appPath),
    attemptId: context.attemptId,
    commit: context.commit,
    journalPath: context.journalPath,
    tag: context.tag,
  };
}

function record(context: StapleContext, status: "started" | "passed" | "failed") {
  const descriptor = openSync(
    context.journalPath,
    constants.O_APPEND | constants.O_CREAT | constants.O_WRONLY | constants.O_NOFOLLOW,
    0o700,
  );
  try {
    writeSync(descriptor, JSON.stringify({ ...context, status }) + "\n");
  } finally {
    closeSync(descriptor);
  }
}

export async function runReleaseXcrun(
  args: readonly string[],
  context: StapleContext,
  ports: XcrunPorts = {
    run: (toolArgs, quiet, timeoutMs) =>
      runXcrunCommand("/usr/bin/xcrun", toolArgs, quiet, timeoutMs),
    wait: async (milliseconds) => {
      await setTimeout(milliseconds);
    },
  },
) {
  const target = args.at(-1);
  if (
    args[0] !== "stapler" ||
    args[1] !== "staple" ||
    target === undefined ||
    resolve(target) !== context.appPath
  ) {
    return ports.run(args, false).status ?? 1;
  }
  record(context, "started");
  for (let attempt = 0; attempt <= retryDelaysMs.length; attempt += 1) {
    const staple = ports.run(args, true, commandTimeoutMs);
    if (staple.status === 0 && !staple.timedOut) {
      const validation = ports.run(
        ["stapler", "validate", context.appPath],
        true,
        commandTimeoutMs,
      );
      if (validation.status === 0 && !validation.timedOut) {
        record(context, "passed");
        return 0;
      }
    }
    const delay = retryDelaysMs[attempt];
    if (delay !== undefined) await ports.wait(delay);
  }
  record(context, "failed");
  return 1;
}

export function runXcrunCommand(
  executable: string,
  args: readonly string[],
  quiet: boolean,
  timeoutMs?: number,
): ToolResult {
  const result = spawnSync(executable, [...args], {
    killSignal: "SIGKILL",
    stdio: quiet ? "ignore" : "inherit",
    timeout: timeoutMs,
  });
  const error = result.error as NodeJS.ErrnoException | undefined;
  return { status: result.status, timedOut: error?.code === "ETIMEDOUT" };
}

if (import.meta.main) {
  try {
    const context = parseStapleContext(process.env[stapleContextEnvironmentName] ?? "");
    process.exitCode = await runReleaseXcrun(process.argv.slice(2), context);
  } catch {
    console.error("Release app stapling failed.");
    process.exitCode = 1;
  }
}
