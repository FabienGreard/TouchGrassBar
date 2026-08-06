type DevelopmentEntitlementsInput = {
  bundleIdentifier: string;
  teamIdentifier: string;
};

type ProvisioningProfileMetadata = {
  Entitlements?: {
    "com.apple.application-identifier"?: unknown;
    "keychain-access-groups"?: unknown;
  };
  ExpirationDate?: unknown;
  Platform?: unknown;
  TeamIdentifier?: unknown;
};

function parseCodeSigningIdentities(output: string) {
  const pattern = /^\s*\d+\)\s+[0-9A-F]{40}\s+"([^"]+)"\s*$/gmu;
  return [...output.matchAll(pattern)].flatMap((match) =>
    match[1] === undefined ? [] : [match[1]],
  );
}

function signingTeamIdentifier(identity: string) {
  const teamIdentifier = /\(([A-Z0-9]{10})\)$/.exec(identity)?.[1];
  if (!teamIdentifier) {
    throw new Error("The development signing identity has no Apple Team ID.");
  }
  return teamIdentifier;
}

function developmentEntitlements({
  bundleIdentifier,
  teamIdentifier,
}: DevelopmentEntitlementsInput) {
  if (
    bundleIdentifier !== "app.touchgrass.bar" &&
    bundleIdentifier !== "app.touchgrass.bar.dev"
  ) {
    throw new Error("The desktop bundle identifier is invalid.");
  }
  if (!/^[A-Z0-9]{10}$/.test(teamIdentifier)) {
    throw new Error("The development Apple Team ID is invalid.");
  }
  const applicationIdentifier = `${teamIdentifier}.${bundleIdentifier}`;
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.application-identifier</key>
  <string>${applicationIdentifier}</string>
  <key>com.apple.developer.team-identifier</key>
  <string>${teamIdentifier}</string>
  <key>keychain-access-groups</key>
  <array>
    <string>${applicationIdentifier}</string>
  </array>
</dict>
</plist>
`;
}

function provisioningProfileAllows(
  profile: ProvisioningProfileMetadata,
  {
    bundleIdentifier,
    teamIdentifier,
  }: DevelopmentEntitlementsInput,
) {
  const applicationIdentifier = `${teamIdentifier}.${bundleIdentifier}`;
  const profileTeams = profile.TeamIdentifier;
  const platforms = profile.Platform;
  const keychainGroups = profile.Entitlements?.["keychain-access-groups"];
  const expiration =
    typeof profile.ExpirationDate === "string"
      ? Date.parse(profile.ExpirationDate)
      : Number.NaN;
  return (
    Number.isFinite(expiration) &&
    expiration > Date.now() &&
    Array.isArray(profileTeams) &&
    profileTeams.includes(teamIdentifier) &&
    Array.isArray(platforms) &&
    platforms.includes("OSX") &&
    profile.Entitlements?.["com.apple.application-identifier"] ===
      applicationIdentifier &&
    Array.isArray(keychainGroups) &&
    keychainGroups.some(
      (group) =>
        group === applicationIdentifier || group === `${teamIdentifier}.*`,
    )
  );
}

export {
  developmentEntitlements,
  parseCodeSigningIdentities,
  provisioningProfileAllows,
  signingTeamIdentifier,
};
export type { DevelopmentEntitlementsInput, ProvisioningProfileMetadata };
