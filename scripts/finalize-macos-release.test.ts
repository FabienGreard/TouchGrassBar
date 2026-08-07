import { describe, expect, test } from "vitest";

import {
  createTrustReceipt,
  formatChecksums,
  parseCodeSignatureDetails,
  validateUpdaterArchiveEntries,
} from "./finalize-macos-release";

const identity = "Developer ID Application: TouchGrassBar (A1B2C3D4E5)";
const validSignature = `Identifier=app.touchgrass.bar
CodeDirectory flags=0x10000(runtime)
Authority=${identity}
Timestamp=Aug 7, 2026 at 12:00:00
TeamIdentifier=A1B2C3D4E5
CDHash=0123456789abcdef0123456789abcdef01234567`;

describe("release artifact trust", () => {
  test("requires the complete public app signature facts", () => {
    expect(parseCodeSignatureDetails(validSignature, identity)).toMatchObject({
      hardenedRuntime: true,
      identity,
      teamIdentifier: "A1B2C3D4E5",
      timestamped: true,
    });
    expect(() =>
      parseCodeSignatureDetails(
        validSignature.replace("CodeDirectory flags=0x10000(runtime)\n", ""),
        identity,
      ),
    ).toThrow("The app signature trust contract is incomplete.");
  });

  test("allows one complete app tree and rejects unsafe archive entries", () => {
    expect(() =>
      validateUpdaterArchiveEntries([
        "TouchGrassBar.app/Contents/Info.plist",
        "TouchGrassBar.app/Contents/MacOS/touchgrassbar",
      ]),
    ).not.toThrow();
    for (const entries of [
      ["/TouchGrassBar.app/Contents/Info.plist"],
      ["TouchGrassBar.app/../outside"],
      ["TouchGrassBar.app/Contents/Info.plist", "other.txt"],
      ["TouchGrassBar.app/Contents/Info.plist"],
    ]) {
      expect(() => validateUpdaterArchiveEntries(entries)).toThrow(
        "Updater archive contents are invalid.",
      );
    }
  });

  test("sorts public checksum records", () => {
    expect(
      formatChecksums([
        { bytes: 2, name: "z.sig", sha256: "b".repeat(64) },
        { bytes: 1, name: "a.dmg", sha256: "a".repeat(64) },
      ]),
    ).toBe(`${"a".repeat(64)}  a.dmg\n${"b".repeat(64)}  z.sig\n`);
  });

  test("creates a sanitized PASS receipt", () => {
    const receipt = createTrustReceipt({
      actions: [{ action: "actions/checkout", ref: "v4" }],
      artifacts: [{ bytes: 123, name: "app.dmg", sha256: "b".repeat(64) }],
      capturedAt: "2026-08-07T12:00:00.000Z",
      certificateSha256: "c".repeat(64),
      commit: "d".repeat(40),
      configuration: { protected_values_emitted: false },
      governance: { schema_version: "touchgrass.release-governance.v1" },
      governanceSha256: "e".repeat(64),
      identity,
      mainCiRunId: "123",
      tag: "v1.2.3",
      teamIdentifier: "A1B2C3D4E5",
      toolchains: {
        bun: "1.3.14",
        cargo: "cargo 1.85.1",
        notarytool: "notarytool version 1",
        rustc: "rustc 1.85.1",
        xcode: "Xcode 16",
      },
      workflowRunId: "456",
    });

    expect(receipt.distribution_trust).toMatchObject({
      app: { notarization: "PASS", stapling: "PASS" },
      dmg: { notarization: "PASS", stapling: "PASS" },
      updater_signature: "PASS",
    });
    expect(receipt.redaction).toEqual({
      credential_material: "ABSENT",
      private_paths: "ABSENT",
      raw_provider_responses: "ABSENT",
      runner_paths: "ABSENT",
    });
    expect(JSON.stringify(receipt)).not.toMatch(
      /APPLE_|TAURI_SIGNING|\/Users\/|\/private\/|runner\/work/,
    );
  });
});
