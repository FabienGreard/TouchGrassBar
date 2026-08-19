import { execFileSync } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

import {
  parseCodeSigningIdentities,
  provisioningProfileAllows,
  signingTeamIdentifier,
  type ProvisioningProfileMetadata,
} from "../apps/desktop/src/dev/dev-signing";

type DevelopmentSigningConfiguration = {
  identity: string;
  provisioningProfile: string;
};

function installedSigningIdentities() {
  return parseCodeSigningIdentities(
    execFileSync("/usr/bin/security", ["find-identity", "-v", "-p", "codesigning"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }),
  );
}

function resolveDevelopmentSigningIdentity(environment: Record<string, string | undefined>) {
  const identities = installedSigningIdentities();
  const configured = environment.TOUCHGRASS_DEV_SIGNING_IDENTITY?.trim();
  if (configured) {
    if (!identities.includes(configured)) {
      throw new Error("The configured development signing identity is not valid.");
    }
    return configured;
  }
  if (identities.length === 1) return identities[0]!;
  if (identities.length === 0) {
    throw new Error("No valid macOS development signing identity is installed.");
  }
  throw new Error(
    "Set TOUCHGRASS_DEV_SIGNING_IDENTITY because multiple code-signing identities are available.",
  );
}

function provisioningProfileCandidates(environment: Record<string, string | undefined>) {
  const configured = environment.TOUCHGRASS_DEV_PROVISIONING_PROFILE?.trim();
  if (configured) return [configured];
  return [
    join(homedir(), "Library", "MobileDevice", "Provisioning Profiles"),
    join(homedir(), "Library", "Developer", "Xcode", "UserData", "Provisioning Profiles"),
  ].flatMap((directory) => {
    if (!existsSync(directory)) return [];
    return readdirSync(directory, { withFileTypes: true })
      .filter(
        (entry) =>
          entry.isFile() &&
          (entry.name.endsWith(".provisionprofile") || entry.name.endsWith(".mobileprovision")),
      )
      .map((entry) => join(directory, entry.name));
  });
}

function provisioningProfileMetadata(path: string) {
  try {
    const decoded = execFileSync("/usr/bin/security", ["cms", "-D", "-i", path], {
      stdio: ["ignore", "pipe", "ignore"],
    });
    const value = (keyPath: string) =>
      execFileSync("/usr/bin/plutil", ["-extract", keyPath, "raw", "-o", "-", "-"], {
        encoding: "utf8",
        input: decoded,
        stdio: ["pipe", "pipe", "ignore"],
      }).trim();
    return {
      Entitlements: {
        "com.apple.application-identifier": value(
          "Entitlements.com\\.apple\\.application-identifier",
        ),
        "keychain-access-groups": [value("Entitlements.keychain-access-groups.0")],
      },
      ExpirationDate: value("ExpirationDate"),
      Platform: [value("Platform.0")],
      TeamIdentifier: [value("TeamIdentifier.0")],
    } satisfies ProvisioningProfileMetadata;
  } catch {
    return null;
  }
}

function resolveDevelopmentSigningConfiguration(
  bundleIdentifier: string,
  environment: Record<string, string | undefined>,
): DevelopmentSigningConfiguration {
  const identity = resolveDevelopmentSigningIdentity(environment);
  const teamIdentifier = signingTeamIdentifier(identity);
  const matches = provisioningProfileCandidates(environment).filter((path) => {
    const profile = provisioningProfileMetadata(path);
    return (
      profile !== null && provisioningProfileAllows(profile, { bundleIdentifier, teamIdentifier })
    );
  });
  if (matches.length === 0) {
    throw new Error(
      `No valid installed macOS provisioning profile authorizes ${bundleIdentifier}.`,
    );
  }
  if (matches.length > 1) {
    throw new Error(
      "Set TOUCHGRASS_DEV_PROVISIONING_PROFILE because multiple matching profiles are installed.",
    );
  }
  return { identity, provisioningProfile: matches[0]! };
}

export { resolveDevelopmentSigningConfiguration };
export type { DevelopmentSigningConfiguration };
