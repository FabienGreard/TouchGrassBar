import { spawnSync } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

const APP_IDENTIFIER = "app.touchgrass.bar";
const APP_NAME = "TouchGrassBar";

type CleanupTarget = {
  description: string;
  path: string;
};

function printHelp() {
  console.log(`Reset TouchGrassBar's local macOS state.

Usage:
  bun run desktop:reset-local [--dry-run] [--uninstall]

Options:
  --dry-run     Print existing targets without removing them.
  --uninstall   Also remove the LaunchAgent and installed application.
  --help        Show this help.

The default reset removes only TouchGrassBar runtime state, WebKit data,
caches, preferences, and saved window state. It does not remove repository
build artifacts. Use "bun run desktop:clean" for Rust build artifacts.`);
}

function isTouchGrassBarRunning() {
  return [APP_NAME, APP_NAME.toLowerCase()].some((processName) => {
    const result = spawnSync("pgrep", ["-x", processName], {
      stdio: "ignore",
    });
    return result.status === 0;
  });
}

function main() {
  const argumentsSet = new Set(process.argv.slice(2));
  const supportedArguments = new Set(["--dry-run", "--help", "--uninstall"]);
  const unknownArguments = [...argumentsSet].filter(
    (argument) => !supportedArguments.has(argument),
  );

  if (unknownArguments.length > 0) {
    console.error(`Unknown argument(s): ${unknownArguments.join(", ")}`);
    printHelp();
    process.exit(1);
  }

  if (argumentsSet.has("--help")) {
    printHelp();
    return;
  }

  if (process.platform !== "darwin") {
    console.error("desktop:reset-local currently supports macOS only.");
    process.exit(1);
  }

  const userHome = homedir();
  if (!userHome || userHome === "/") {
    console.error("Could not resolve a safe user home directory.");
    process.exit(1);
  }

  const runtimeTargets: CleanupTarget[] = [
    {
      description: "application state and lifecycle database",
      path: join(userHome, "Library", "Application Support", APP_IDENTIFIER),
    },
    {
      description: "application cache",
      path: join(userHome, "Library", "Caches", APP_IDENTIFIER),
    },
    {
      description: "legacy development cache",
      path: join(userHome, "Library", "Caches", APP_NAME.toLowerCase()),
    },
    {
      description: "WebKit data",
      path: join(userHome, "Library", "WebKit", APP_IDENTIFIER),
    },
    {
      description: "legacy development WebKit data",
      path: join(userHome, "Library", "WebKit", APP_NAME.toLowerCase()),
    },
    {
      description: "HTTP storage",
      path: join(userHome, "Library", "HTTPStorages", APP_IDENTIFIER),
    },
    {
      description: "cookies",
      path: join(
        userHome,
        "Library",
        "Cookies",
        `${APP_IDENTIFIER}.binarycookies`,
      ),
    },
    {
      description: "preferences",
      path: join(userHome, "Library", "Preferences", `${APP_IDENTIFIER}.plist`),
    },
    {
      description: "saved window state",
      path: join(
        userHome,
        "Library",
        "Saved Application State",
        `${APP_IDENTIFIER}.savedState`,
      ),
    },
  ];

  const uninstallTargets: CleanupTarget[] = [
    {
      description: "launch-at-login agent",
      path: join(userHome, "Library", "LaunchAgents", `${APP_NAME}.plist`),
    },
    {
      description: "system Applications install",
      path: join("/Applications", `${APP_NAME}.app`),
    },
    {
      description: "user Applications install",
      path: join(userHome, "Applications", `${APP_NAME}.app`),
    },
  ];

  const targets = argumentsSet.has("--uninstall")
    ? [...runtimeTargets, ...uninstallTargets]
    : runtimeTargets;
  const allowedPaths = new Set(
    [...runtimeTargets, ...uninstallTargets].map((target) =>
      resolve(target.path),
    ),
  );

  for (const target of targets) {
    if (!allowedPaths.has(resolve(target.path))) {
      throw new Error(`Refusing unexpected cleanup target: ${target.path}`);
    }
  }

  const existingTargets = targets.filter((target) => existsSync(target.path));
  const dryRun = argumentsSet.has("--dry-run");

  if (!dryRun && isTouchGrassBarRunning()) {
    console.error(
      `Quit ${APP_NAME} completely from its menu-bar menu before resetting local state.`,
    );
    process.exit(1);
  }

  if (existingTargets.length === 0) {
    console.log("No TouchGrassBar local state matched the cleanup plan.");
    return;
  }

  for (const target of existingTargets) {
    if (dryRun) {
      console.log(`[dry-run] ${target.description}: ${target.path}`);
      continue;
    }

    rmSync(target.path, { force: true, recursive: true });
    console.log(`Removed ${target.description}: ${target.path}`);
  }

  if (!dryRun) {
    console.log(
      argumentsSet.has("--uninstall")
        ? "TouchGrassBar local state and installations were removed."
        : [
            "TouchGrassBar local state was reset.",
            "The onboarding will open on the next manual launch.",
          ].join(" "),
    );
  }
}

main();
