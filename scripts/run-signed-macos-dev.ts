#!/usr/bin/env bun

import { execFileSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  rmSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";

function requiredEnvironment(name: string) {
  const value = Bun.env[name]?.trim();
  if (!value) throw new Error(`Missing ${name} for macOS development signing.`);
  return value;
}

const [executable, ...argumentsList] = Bun.argv.slice(2);
if (!executable) throw new Error("Cargo did not provide a development executable.");

const appBundlePath = resolve(requiredEnvironment("TOUCHGRASS_DEV_APP_BUNDLE_PATH"));
const generatedDirectory = resolve(
  import.meta.dir,
  "..",
  "apps",
  "desktop",
  "src-tauri",
  ".dev-instance",
);
if (
  dirname(appBundlePath) !== generatedDirectory ||
  !appBundlePath.endsWith(".app")
) {
  throw new Error("The development app bundle path is outside the generated directory.");
}

const contentsPath = join(appBundlePath, "Contents");
const executableDirectory = join(contentsPath, "MacOS");
const bundledExecutable = join(executableDirectory, "touchgrassbar");
rmSync(appBundlePath, { force: true, recursive: true });
mkdirSync(executableDirectory, { recursive: true });
copyFileSync(resolve(executable), bundledExecutable);
chmodSync(bundledExecutable, 0o755);
copyFileSync(
  resolve(requiredEnvironment("TOUCHGRASS_DEV_INFO_PLIST_PATH")),
  join(contentsPath, "Info.plist"),
);
copyFileSync(
  resolve(requiredEnvironment("TOUCHGRASS_DEV_PROVISIONING_PROFILE")),
  join(contentsPath, "embedded.provisionprofile"),
);

const signingArguments = [
  "--force",
  "--timestamp=none",
  "--sign",
  requiredEnvironment("TOUCHGRASS_DEV_SIGNING_IDENTITY"),
  "--entitlements",
  resolve(requiredEnvironment("TOUCHGRASS_DEV_ENTITLEMENTS_PATH")),
  appBundlePath,
];
execFileSync("/usr/bin/codesign", signingArguments, { stdio: "inherit" });
execFileSync(
  "/usr/bin/codesign",
  ["--verify", "--deep", "--strict", appBundlePath],
  { stdio: "inherit" },
);

console.log(`Running signed development app: ${basename(appBundlePath)}`);
// Replace the runner so a Tauri rebuild stops the app instead of orphaning it.
process.execve(
  bundledExecutable,
  [bundledExecutable, ...argumentsList],
  process.env,
);
