import { spawn, spawnSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { expect, test } from "vitest";

const workspaceRoot = resolve(import.meta.dirname, "../../../..");
const runnerPath = join(workspaceRoot, "scripts", "run-signed-macos-dev.ts");
const generatedDirectory = join(
  workspaceRoot,
  "apps",
  "desktop",
  "src-tauri",
  ".dev-instance",
);

function runningProcessIds(executable: string) {
  const result = spawnSync("/bin/ps", ["-axo", "pid=,command="], {
    encoding: "utf8",
  });
  if (result.status !== 0) throw new Error("Process inspection failed.");
  return result.stdout
    .split("\n")
    .map((line) => /^\s*(\d+)\s+(.*)$/.exec(line))
    .filter((match): match is RegExpExecArray => match !== null)
    .filter((match) => {
      const command = match[2] ?? "";
      return command === executable || command.startsWith(`${executable} `);
    })
    .map((match) => Number(match[1]));
}

async function waitFor(
  predicate: () => boolean,
  timeoutMilliseconds = 5_000,
) {
  const deadline = Date.now() + timeoutMilliseconds;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolveWait) => setTimeout(resolveWait, 25));
  }
  throw new Error("Timed out while waiting for the signed development app.");
}

test.runIf(process.platform === "darwin")(
  "a forced runner restart cannot leave the signed development app alive",
  async () => {
    const fixtureDirectory = mkdtempSync(
      join(tmpdir(), "touchgrassbar-signed-runner-"),
    );
    const appBundlePath = join(
      generatedDirectory,
      `TouchGrassBar Runner Test ${process.pid}.app`,
    );
    const bundledExecutable = join(
      appBundlePath,
      "Contents",
      "MacOS",
      "touchgrassbar",
    );
    const infoPlistPath = join(fixtureDirectory, "Info.plist");
    const entitlementsPath = join(fixtureDirectory, "entitlements.plist");
    const provisioningProfilePath = join(
      fixtureDirectory,
      "embedded.provisionprofile",
    );
    const fixtureSourcePath = join(fixtureDirectory, "fixture.c");
    const fixtureExecutablePath = join(fixtureDirectory, "fixture");

    mkdirSync(generatedDirectory, { recursive: true });
    writeFileSync(
      infoPlistPath,
      `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>touchgrassbar</string>
<key>CFBundleIdentifier</key><string>app.touchgrass.bar.dev.runner-test</string>
<key>CFBundleName</key><string>TouchGrassBar Runner Test</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
</dict></plist>
`,
    );
    writeFileSync(
      entitlementsPath,
      '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict/></plist>\n',
    );
    writeFileSync(provisioningProfilePath, "test-profile\n");
    writeFileSync(
      fixtureSourcePath,
      "#include <unistd.h>\nint main(void) { sleep(60); return 0; }\n",
    );
    const compile = spawnSync(
      "/usr/bin/clang",
      [fixtureSourcePath, "-o", fixtureExecutablePath],
      { encoding: "utf8" },
    );
    if (compile.status !== 0) {
      throw new Error(`Fixture compilation failed:\n${compile.stderr}`);
    }

    const runner = spawn("bun", [runnerPath, fixtureExecutablePath], {
      cwd: workspaceRoot,
      env: {
        ...process.env,
        TOUCHGRASS_DEV_APP_BUNDLE_PATH: appBundlePath,
        TOUCHGRASS_DEV_ENTITLEMENTS_PATH: entitlementsPath,
        TOUCHGRASS_DEV_INFO_PLIST_PATH: infoPlistPath,
        TOUCHGRASS_DEV_PROVISIONING_PROFILE: provisioningProfilePath,
        TOUCHGRASS_DEV_SIGNING_IDENTITY: "-",
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let runnerOutput = "";
    runner.stdout.on("data", (chunk: Buffer) => {
      runnerOutput += chunk.toString();
    });
    runner.stderr.on("data", (chunk: Buffer) => {
      runnerOutput += chunk.toString();
    });

    try {
      await waitFor(() => {
        if (runner.exitCode !== null) {
          throw new Error(`Signed runner exited early:\n${runnerOutput}`);
        }
        return runningProcessIds(bundledExecutable).length === 1;
      });
      runner.kill("SIGKILL");
      await new Promise<void>((resolveExit) => runner.once("exit", resolveExit));
      await new Promise((resolveWait) => setTimeout(resolveWait, 100));

      expect(runningProcessIds(bundledExecutable)).toEqual([]);
    } finally {
      for (const processId of runningProcessIds(bundledExecutable)) {
        try {
          process.kill(processId, "SIGKILL");
        } catch {
          // The process already exited.
        }
      }
      rmSync(appBundlePath, { force: true, recursive: true });
      rmSync(fixtureDirectory, { force: true, recursive: true });
    }
  },
  10_000,
);
