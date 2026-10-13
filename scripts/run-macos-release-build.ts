import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
  chmodSync,
  closeSync,
  constants,
  fstatSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  parseStapleContext,
  stapleContextEnvironmentName,
  type StapleContext,
} from "./run-release-xcrun";

type BuildOptions = { cwd: string; env: NodeJS.ProcessEnv };
type ReleaseBuildInput = {
  environment?: NodeJS.ProcessEnv;
  root?: string;
  run?: (command: readonly string[], options: BuildOptions) => Promise<number>;
  temporaryRoot?: string;
};
const workspaceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
function shellQuote(value: string) {
  return "'" + value.replaceAll("'", "'\\''") + "'";
}
async function execute(command: readonly string[], options: BuildOptions) {
  const child = spawn(command[0]!, command.slice(1), {
    ...options,
    detached: true,
    stdio: "inherit",
  });
  let interrupted = false;
  let forcedStop: Promise<void> | undefined;
  const stop = () => {
    interrupted = true;
    if (child.pid === undefined) return;
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      /* Already stopped. */
    }
    forcedStop ??= new Promise<void>((done) => {
      setTimeout(() => {
        try {
          process.kill(-child.pid!, "SIGKILL");
        } catch {
          /* Already stopped. */
        }
        done();
      }, 2_000);
    });
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
  try {
    return await new Promise<number>((done, reject) => {
      child.on("error", reject);
      child.on("close", (code) => done(interrupted ? 1 : (code ?? 1)));
    });
  } finally {
    // The leader can exit before its descendants. Complete escalation before cleanup.
    if (forcedStop !== undefined) await forcedStop;
    process.off("SIGINT", stop);
    process.off("SIGTERM", stop);
  }
}

function requireStapleProof(context: StapleContext) {
  try {
    const descriptor = openSync(context.journalPath, constants.O_RDONLY | constants.O_NOFOLLOW);
    let text: string;
    try {
      const metadata = fstatSync(descriptor);
      if (!metadata.isFile() || metadata.size > 65_536 || (metadata.mode & 0o777) !== 0o700)
        throw new Error();
      text = readFileSync(descriptor, "utf8");
    } finally {
      closeSync(descriptor);
    }
    const entries = text.trim().split("\n");
    if (entries.length > 64) throw new Error();
    let started = 0;
    let passed = 0;
    for (const line of entries) {
      const entry = JSON.parse(line) as StapleContext & { status: string };
      if (
        entry.appPath !== context.appPath ||
        entry.attemptId !== context.attemptId ||
        entry.commit !== context.commit ||
        entry.tag !== context.tag ||
        entry.journalPath !== context.journalPath
      )
        throw new Error();
      if (entry.status === "started") started += 1;
      else if (entry.status === "passed") passed += 1;
      else throw new Error();
    }
    if (started === 0 || started !== passed) throw new Error();
  } catch {
    throw new Error("The macOS release app staple proof failed.");
  }
}
export async function runMacosReleaseBuild(input: ReleaseBuildInput = {}) {
  const root = input.root ?? workspaceRoot;
  const environment = input.environment ?? process.env;
  const temporary = mkdtempSync(
    join(input.temporaryRoot ?? environment.RUNNER_TEMP ?? tmpdir(), "tgb-release-staple-"),
  );
  chmodSync(temporary, 0o700);
  try {
    const context = parseStapleContext(
      JSON.stringify({
        appPath: join(root, "apps/desktop/src-tauri/target/release/bundle/macos/TouchGrassBar.app"),
        attemptId: randomUUID(),
        commit: environment.RELEASE_COMMIT,
        journalPath: join(temporary, "journal"),
        tag: environment.RELEASE_TAG,
      }),
    );
    writeFileSync(
      join(temporary, "xcrun"),
      "#!/bin/sh\nexec " +
        shellQuote(process.execPath) +
        " --no-env-file " +
        shellQuote(join(workspaceRoot, "scripts/run-release-xcrun.ts")) +
        ' "$@"\n',
      { mode: 0o700 },
    );
    const childEnvironment = {
      ...environment,
      PATH: temporary + delimiter + (environment.PATH ?? ""),
      [stapleContextEnvironmentName]: JSON.stringify(context),
    };
    const exitCode = await (input.run ?? execute)(
      [
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
      ],
      { cwd: root, env: childEnvironment },
    );
    if (exitCode !== 0) throw new Error("The macOS release build failed.");
    requireStapleProof(context);
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  try {
    if (process.platform !== "darwin") throw new Error();
    await runMacosReleaseBuild();
    console.log("Release app staple proof passed.");
  } catch {
    console.error("The macOS release build or app staple proof failed.");
    process.exitCode = 1;
  }
}
