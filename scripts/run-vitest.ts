import { mkdir } from "node:fs/promises";
import { join, resolve } from "node:path";

const suiteByPackage: Record<string, string> = {
  "@touchgrass/backend": "backend",
  "@touchgrass/contracts": "contracts",
  "@touchgrass/desktop": "desktop",
  "@touchgrass/landing": "landing",
  touchgrassbar: "release",
};

type PackageManifest = {
  name?: string;
};

async function packageName(): Promise<string> {
  try {
    const manifest = (await Bun.file(
      join(process.cwd(), "package.json"),
    ).json()) as PackageManifest;
    return manifest.name ?? "vitest";
  } catch {
    return "vitest";
  }
}

const suiteId =
  process.env.TOUCHGRASS_VITEST_SUITE_ID ?? suiteByPackage[await packageName()] ?? "vitest";
const childEnvironment = { ...process.env };

if (process.env.GITHUB_ACTIONS === "true" && process.env.GITHUB_STEP_SUMMARY) {
  const reportDirectory = resolve(import.meta.dir, "..", ".vitest-reports");
  await mkdir(reportDirectory, { recursive: true });
  const reportPath = join(reportDirectory, `${suiteId}.md`);
  await Bun.write(reportPath, "");
  childEnvironment.GITHUB_STEP_SUMMARY = reportPath;
}

const vitest = Bun.spawn(["bunx", "vitest", "run", ...process.argv.slice(2)], {
  cwd: process.cwd(),
  env: childEnvironment,
  stderr: "inherit",
  stdin: "inherit",
  stdout: "inherit",
});

process.exitCode = await vitest.exited;
