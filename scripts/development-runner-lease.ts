import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";

import { workspaceRoot } from "./development-environment";

const developmentRunnerPath = join(
  workspaceRoot,
  ".convex",
  "touchgrass-dev-runner.pid",
);

function processIsRunning(processId: number) {
  try {
    process.kill(processId, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code !== "ESRCH";
  }
}

function verifiedDevelopmentRunner(processId: number) {
  const command = execFileSync(
    "/bin/ps",
    ["-p", String(processId), "-o", "command="],
    { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
  ).trim();
  if (!command.includes("scripts/run-dev.ts")) return false;
  const workingDirectoryOutput = execFileSync(
    "lsof",
    ["-a", "-p", String(processId), "-d", "cwd", "-Fn"],
    { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
  );
  const workingDirectory = workingDirectoryOutput
    .split(/\r?\n/)
    .find((line) => line.startsWith("n"))
    ?.slice(1);
  return (
    workingDirectory !== undefined &&
    resolve(workingDirectory) === workspaceRoot
  );
}

function activeDevelopmentRunnerProcessId() {
  if (!existsSync(developmentRunnerPath)) return null;
  const processId = Number(readFileSync(developmentRunnerPath, "utf8"));
  if (!Number.isInteger(processId) || processId <= 0) {
    unlinkSync(developmentRunnerPath);
    return null;
  }
  if (!processIsRunning(processId)) {
    unlinkSync(developmentRunnerPath);
    return null;
  }
  let verified = false;
  try {
    verified = verifiedDevelopmentRunner(processId);
  } catch {
    throw new Error("The active development command could not be verified.");
  }
  if (!verified) {
    unlinkSync(developmentRunnerPath);
    return null;
  }
  return processId;
}

function claimDevelopmentRunnerLease() {
  if (activeDevelopmentRunnerProcessId() !== null) {
    throw new Error("This worktree already has an active development command.");
  }
  mkdirSync(join(workspaceRoot, ".convex"), { recursive: true });
  try {
    writeFileSync(developmentRunnerPath, String(process.pid), {
      flag: "wx",
      mode: 0o600,
    });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "EEXIST") {
      throw new Error(
        "This worktree already has an active development command.",
        { cause: error },
      );
    }
    throw error;
  }
}

function releaseDevelopmentRunnerLease(expectedProcessId: number) {
  try {
    if (
      readFileSync(developmentRunnerPath, "utf8") ===
      String(expectedProcessId)
    ) {
      unlinkSync(developmentRunnerPath);
    }
  } catch {
    // Reset or a previous shutdown hook may already have removed the lease.
  }
}

export {
  activeDevelopmentRunnerProcessId,
  claimDevelopmentRunnerLease,
  developmentRunnerPath,
  processIsRunning,
  releaseDevelopmentRunnerLease,
};
