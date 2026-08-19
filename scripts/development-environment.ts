import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const localEnvironmentPath = join(workspaceRoot, ".env.local");
const profileServiceEnvironmentNames = ["CONVEX_SITE_URL", "CONVEX_URL"] as const;

type DevelopmentTarget = "cloud development" | "cloud production" | "local";

function readLocalDevelopmentEnvironment() {
  if (!existsSync(localEnvironmentPath)) return {};
  return Object.fromEntries(
    readFileSync(localEnvironmentPath, "utf8")
      .split(/\r?\n/)
      .flatMap((line) => {
        const match = /^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)\s*$/.exec(line);
        if (!match) return [];
        const value = match[2] ?? "";
        return [[match[1]!, value.replace(/^(['"])(.*)\1$/, "$2")]];
      }),
  ) as Record<string, string>;
}

function requiredEnvironment(
  environment: Record<string, string | undefined>,
  names: readonly string[],
) {
  const missing = names.filter((name) => !environment[name]?.trim());
  if (missing.length > 0) {
    throw new Error(
      `Development environment is incomplete (${missing.join(", ")}). Run \`bun setup\` or select a Convex deployment.`,
    );
  }
}

function validServiceUrl(
  name: (typeof profileServiceEnvironmentNames)[number],
  value: string,
  target: DevelopmentTarget,
) {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error("A Convex service URL is invalid.");
  }
  if (target === "local") {
    if (
      url.protocol !== "http:" ||
      (url.hostname !== "127.0.0.1" && url.hostname !== "localhost")
    ) {
      throw new Error("The selected local deployment has a non-local URL.");
    }
    return;
  }
  const expectedSuffix = name === "CONVEX_URL" ? ".convex.cloud" : ".convex.site";
  if (url.protocol !== "https:" || !url.hostname.endsWith(expectedSuffix)) {
    throw new Error("The selected cloud deployment has an invalid URL.");
  }
}

function developmentTarget(environment: Record<string, string | undefined>): DevelopmentTarget {
  requiredEnvironment(environment, ["CONVEX_DEPLOYMENT", ...profileServiceEnvironmentNames]);
  const deployment = environment.CONVEX_DEPLOYMENT!.trim();
  const convexUrl = environment.CONVEX_URL!.trim();
  const siteUrl = environment.CONVEX_SITE_URL!.trim();
  const selectedCloudTarget = environment.TOUCHGRASS_CONVEX_TARGET?.trim();
  const target: DevelopmentTarget =
    deployment.startsWith("anonymous:") || deployment.startsWith("local:")
      ? "local"
      : deployment.startsWith("dev:") || selectedCloudTarget === "dev"
        ? "cloud development"
        : selectedCloudTarget === "prod"
          ? "cloud production"
          : (() => {
              throw new Error("The selected Convex deployment type is unknown.");
            })();
  validServiceUrl("CONVEX_URL", convexUrl, target);
  validServiceUrl("CONVEX_SITE_URL", siteUrl, target);
  return target;
}

export {
  developmentTarget,
  localEnvironmentPath,
  profileServiceEnvironmentNames,
  readLocalDevelopmentEnvironment,
  requiredEnvironment,
  workspaceRoot,
};
export type { DevelopmentTarget };
