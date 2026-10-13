import { describe, expect, test } from "vitest";

import {
  createTrustReceipt,
  formatChecksums,
  parseCodeSignatureDetails,
  requireStapledDmg,
  requireValidStapledTicket,
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
  test.each([68, 69, 75])(
    "temporary DMG staple exit %s can recover before strict validation",
    (status) => {
      let staples = 0;
      let validations = 0;
      const waits: number[] = [];
      expect(() =>
        requireStapledDmg(
          () => ({ status: ++staples === 1 ? status : 0 }),
          () => {
            validations += 1;
            return { status: 0 };
          },
          (delay) => waits.push(delay),
        ),
      ).not.toThrow();
      expect(staples).toBe(2);
      expect(validations).toBe(1);
      expect(waits).toEqual([20_000]);
    },
  );

  test("DMG stapling stops after three temporary failures and never validates", () => {
    let staples = 0;
    let validations = 0;
    const waits: number[] = [];
    expect(() =>
      requireStapledDmg(
        () => {
          staples += 1;
          return { status: 69 };
        },
        () => {
          validations += 1;
          return { status: 0 };
        },
        (delay) => waits.push(delay),
      ),
    ).toThrow("The DMG ticket cannot be stapled. (staple exit 69).");
    expect(staples).toBe(3);
    expect(validations).toBe(0);
    expect(waits).toEqual([20_000, 40_000]);
  });

  test.each([null, 0])(
    "a timed-out DMG staple with exit %s is retried and cannot authorize validation",
    (status) => {
      let staples = 0;
      let validations = 0;
      requireStapledDmg(
        () => (++staples === 1 ? { status, timedOut: true } : { status: 0 }),
        () => {
          validations += 1;
          return { status: 0 };
        },
        () => undefined,
      );
      expect(staples).toBe(2);
      expect(validations).toBe(1);
    },
  );

  test("exhausted DMG staple timeouts fail closed even with exit zero", () => {
    let staples = 0;
    let validations = 0;
    expect(() =>
      requireStapledDmg(
        () => {
          staples += 1;
          return { status: 0, timedOut: true };
        },
        () => {
          validations += 1;
          return { status: 0 };
        },
        () => undefined,
      ),
    ).toThrow("The DMG ticket cannot be stapled. (staple timed out).");
    expect(staples).toBe(3);
    expect(validations).toBe(0);
  });

  test.each([1, 65, 66, 77, null])(
    "invalid or unavailable DMG staple exit %s stops immediately",
    (status) => {
      let staples = 0;
      let validations = 0;
      const waits: number[] = [];
      expect(() =>
        requireStapledDmg(
          () => {
            staples += 1;
            return { status };
          },
          () => {
            validations += 1;
            return { status: 0 };
          },
          (delay) => waits.push(delay),
        ),
      ).toThrow("The DMG ticket cannot be stapled.");
      expect(staples).toBe(1);
      expect(validations).toBe(0);
      expect(waits).toEqual([]);
    },
  );

  test("a successful DMG staple still requires strict validation and does not staple again during validation retry", () => {
    const operations: string[] = [];
    let validations = 0;
    requireStapledDmg(
      () => {
        operations.push("staple");
        return { status: 0 };
      },
      () => {
        operations.push("validate");
        return { status: ++validations === 1 ? 75 : 0 };
      },
      () => undefined,
    );
    expect(operations).toEqual(["staple", "validate", "validate"]);
  });

  test.each([{ status: 77 }, { status: 0, timedOut: true }])(
    "a DMG staple cannot hide invalid or timed-out validation: %j",
    (result) => {
      let staples = 0;
      expect(() =>
        requireStapledDmg(
          () => {
            staples += 1;
            return { status: 0 };
          },
          () => result,
          () => undefined,
        ),
      ).toThrow("The DMG stapling result is invalid.");
      expect(staples).toBe(1);
    },
  );

  test.each([68, 69, 75])(
    "retries ticket-service exit %s and requires later validation",
    (status) => {
      let attempts = 0;
      const waits: number[] = [];
      expect(() =>
        requireValidStapledTicket(
          () => ({ status: ++attempts === 1 ? status : 0 }),
          "Invalid ticket",
          (delay) => waits.push(delay),
        ),
      ).not.toThrow();
      expect(attempts).toBe(2);
      expect(waits).toEqual([20_000]);
    },
  );

  test("stops after three temporary validation failures", () => {
    let attempts = 0;
    const waits: number[] = [];
    expect(() =>
      requireValidStapledTicket(
        () => {
          attempts += 1;
          return { status: 69 };
        },
        "Invalid ticket",
        (delay) => waits.push(delay),
      ),
    ).toThrow("Invalid ticket");
    expect(attempts).toBe(3);
    expect(waits).toEqual([20_000, 40_000]);
  });

  test("retries a timed-out validation without accepting the timeout", () => {
    let attempts = 0;
    expect(() =>
      requireValidStapledTicket(
        () => (++attempts === 1 ? { status: null, timedOut: true } : { status: 0 }),
        "Invalid ticket",
        () => undefined,
      ),
    ).not.toThrow();
    expect(attempts).toBe(2);
  });

  test("does not accept a timed-out validator that exits zero during termination", () => {
    let attempts = 0;
    requireValidStapledTicket(
      () => (++attempts === 1 ? { status: 0, timedOut: true } : { status: 0 }),
      "Invalid ticket",
      () => undefined,
    );
    expect(attempts).toBe(2);
  });

  test.each([65, 66, 77, null])(
    "rejects invalid or unavailable local ticket state %s",
    (status) => {
      let attempts = 0;
      const waits: number[] = [];
      expect(() =>
        requireValidStapledTicket(
          () => {
            attempts += 1;
            return { status };
          },
          "Invalid ticket",
          (delay) => waits.push(delay),
        ),
      ).toThrow("Invalid ticket");
      expect(attempts).toBe(1);
      expect(waits).toEqual([]);
    },
  );

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
