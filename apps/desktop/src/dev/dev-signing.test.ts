import { describe, expect, test } from "vitest";

import {
  developmentEntitlements,
  parseCodeSigningIdentities,
  provisioningProfileAllows,
  signingTeamIdentifier,
} from "@/dev/dev-signing";

describe("macOS development signing", () => {
  test("builds worktree-specific Keychain entitlements from an Apple identity", () => {
    const identities = parseCodeSigningIdentities(`
  1) 0123456789ABCDEF0123456789ABCDEF01234567 "Developer ID Application: Example Developer (AB12CD34EF)"
     1 valid identities found
`);
    const identity = identities[0];

    expect(identity).toBe(
      "Developer ID Application: Example Developer (AB12CD34EF)",
    );
    const teamIdentifier = signingTeamIdentifier(identity!);
    expect(teamIdentifier).toBe("AB12CD34EF");
    expect(
      developmentEntitlements({
        bundleIdentifier: "app.touchgrass.bar.dev",
        teamIdentifier,
      }),
    ).toContain("<string>AB12CD34EF.app.touchgrass.bar.dev</string>");
    const profile = {
      Entitlements: {
        "com.apple.application-identifier":
          "AB12CD34EF.app.touchgrass.bar.dev",
        "keychain-access-groups": ["AB12CD34EF.*"],
      },
      ExpirationDate: "2999-01-01T00:00:00Z",
      Platform: ["OSX"],
      TeamIdentifier: ["AB12CD34EF"],
    };
    expect(
      provisioningProfileAllows(profile, {
        bundleIdentifier: "app.touchgrass.bar.dev",
        teamIdentifier,
      }),
    ).toBe(true);
    expect(
      provisioningProfileAllows(profile, {
        bundleIdentifier: "app.touchgrass.bar.dev.wexample",
        teamIdentifier,
      }),
    ).toBe(false);
    expect(
      provisioningProfileAllows(
        { ...profile, ExpirationDate: "2000-01-01T00:00:00Z" },
        {
          bundleIdentifier: "app.touchgrass.bar.dev",
          teamIdentifier,
        },
      ),
    ).toBe(false);
    expect(
      developmentEntitlements({
        bundleIdentifier: "app.touchgrass.bar",
        teamIdentifier,
      }),
    ).toContain("<string>AB12CD34EF.app.touchgrass.bar</string>");
  });
});
