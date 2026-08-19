import { appendFile, readFile, readdir } from "node:fs/promises";
import { basename, resolve } from "node:path";

import { parseVitestSummary, renderVitestSummary } from "./vitest-summary";

const summaryPath = process.env.GITHUB_STEP_SUMMARY;
if (!summaryPath) {
  throw new Error("GITHUB_STEP_SUMMARY is not available.");
}

const reportDirectory = resolve(import.meta.dir, "..", ".vitest-reports");
let reportNames: string[] = [];
try {
  reportNames = (await readdir(reportDirectory)).filter((name) => name.endsWith(".md"));
} catch (error) {
  if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
}

const summaries = (
  await Promise.all(
    reportNames.map(async (name) =>
      parseVitestSummary(
        basename(name, ".md"),
        await readFile(resolve(reportDirectory, name), "utf8"),
      ),
    ),
  )
).filter((summary) => summary !== null);

await appendFile(summaryPath, renderVitestSummary(summaries), "utf8");
