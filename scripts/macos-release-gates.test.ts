import { describe, expect, test } from "vitest";

import {
  bindReleaseCandidate,
  buildAutomatedPreflight,
  parseMacosReleaseGateArguments,
  parseApprovedPowerState,
  parsePanelPaintMetric,
  parseReleaseGateDriverEvent,
} from "./macos-release-gates";

const commit = "a".repeat(40);
const dmgSha256 = "b".repeat(64);
const dmgName = "TouchGrassBar_1.2.3_aarch64.dmg";

function trustReceipt() {
  return {
    schema_version: "touchgrass.release-trust.v1",
    candidate: {
      tag: "v1.2.3",
      version: "1.2.3",
      commit,
      main_ci_run_id: "123",
      workflow_run_id: "456",
    },
    artifacts: [{ bytes: 24_000, name: dmgName, sha256: dmgSha256 }],
    distribution_trust: {
      app: {
        architecture: "arm64",
        gatekeeper: "PASS",
        hardened_runtime: "PASS",
        notarization: "PASS",
        stapling: "PASS",
        timestamp: "PASS",
      },
      dmg: {
        gatekeeper: "PASS",
        notarization: "PASS",
        stapling: "PASS",
      },
      updater_signature: "PASS",
    },
    redaction: {
      credential_material: "ABSENT",
      private_paths: "ABSENT",
      raw_provider_responses: "ABSENT",
      runner_paths: "ABSENT",
    },
  };
}

describe("macOS release-gate CLI", () => {
  test("requires one DMG, trust receipt, and output receipt", () => {
    expect(
      parseMacosReleaseGateArguments([
        "--dmg",
        "candidate.dmg",
        "--trust",
        "release-trust.json",
        "--output",
        "performance.json",
      ]),
    ).toEqual({
      dmg: "candidate.dmg",
      output: "performance.json",
      trust: "release-trust.json",
    });

    expect(() => parseMacosReleaseGateArguments([])).toThrow("Usage:");
    expect(() =>
      parseMacosReleaseGateArguments([
        "--dmg",
        "a",
        "--trust",
        "b",
        "--output",
        "c",
        "--unknown",
        "d",
      ]),
    ).toThrow("Usage:");
  });

  test("binds the exact trusted DMG and candidate identity", () => {
    expect(
      bindReleaseCandidate({
        appBytes: 30_000,
        appVersion: "1.2.3",
        dmgBytes: 24_000,
        dmgName,
        dmgSha256,
        receipt: trustReceipt(),
      }),
    ).toEqual({
      candidate: {
        app_bytes: 30_000,
        artifact_sha256: dmgSha256,
        commit,
        dmg_bytes: 24_000,
        version: "1.2.3",
      },
      mainCiRunId: "123",
    });
  });

  test("rejects candidate, artifact, and trust mismatches", () => {
    const cases = [
      { appVersion: "1.2.4" },
      { dmgBytes: 24_001 },
      { dmgSha256: "c".repeat(64) },
      { dmgName: "other.dmg" },
      {
        receipt: {
          ...trustReceipt(),
          artifacts: [...trustReceipt().artifacts, ...trustReceipt().artifacts],
        },
      },
      {
        receipt: {
          ...trustReceipt(),
          redaction: {
            ...trustReceipt().redaction,
            private_paths: "PRESENT",
          },
        },
      },
    ];
    for (const changed of cases) {
      expect(() =>
        bindReleaseCandidate({
          appBytes: 30_000,
          appVersion: "1.2.3",
          dmgBytes: 24_000,
          dmgName,
          dmgSha256,
          receipt: trustReceipt(),
          ...changed,
        }),
      ).toThrow("Release trust receipt does not bind the measured candidate.");
    }
  });

  test("accepts only the sanitized driver and paint metric lines", () => {
    expect(parseReleaseGateDriverEvent("touchgrassbar_release_gate event=menu_bar_ready")).toBe(
      "menu_bar_ready",
    );
    expect(
      parseReleaseGateDriverEvent("touchgrassbar_release_gate event=rapid_interaction_pass"),
    ).toBe("rapid_interaction_pass");
    expect(parseReleaseGateDriverEvent("private/path event=ready")).toBeNull();
    expect(
      parseReleaseGateDriverEvent("touchgrassbar_release_gate event=menu_bar_ready extra=data"),
    ).toBeNull();

    expect(
      parsePanelPaintMetric(
        "touchgrassbar_metric panel_paint_source=synthetic panel_paint_ms=12.375",
      ),
    ).toBe(12.375);
    expect(
      parsePanelPaintMetric("touchgrassbar_metric panel_paint_source=tray panel_paint_ms=12.375"),
    ).toBeNull();
    expect(
      parsePanelPaintMetric("touchgrassbar_metric panel_paint_source=synthetic panel_paint_ms=NaN"),
    ).toBeNull();
    expect(parsePanelPaintMetric("raw provider response")).toBeNull();
  });

  test("accepts AC power only when the active power mode is not low", () => {
    expect(
      parseApprovedPowerState(
        "Now drawing from 'AC Power'",
        "Battery Power:\n powermode 1\nAC Power:\n powermode 0\n sleep 0\n",
      ),
    ).toBe(true);
    expect(
      parseApprovedPowerState("Now drawing from 'AC Power'", "AC Power:\n lowpowermode 1\n"),
    ).toBe(false);
    expect(
      parseApprovedPowerState("Now drawing from 'Battery Power'", "AC Power:\n powermode 0\n"),
    ).toBe(false);
  });

  test("fails only the missing local or exact CI preflight evidence", () => {
    const allLocal = {
      currentSpace: true,
      escape: true,
      outsideClick: true,
      persistedLaunchAtLogin: true,
      positioningClamping: true,
      rapidInteraction: true,
      toggling: true,
    };
    const ci = {
      conclusion: "success",
      headSha: commit,
      jobs: [
        { conclusion: "success", name: "Native app (macOS 15 floor, Apple silicon)" },
        {
          conclusion: "success",
          name: "Native app (macOS 26 latest stable, Apple silicon)",
        },
      ],
    };
    expect(buildAutomatedPreflight(allLocal, ci, commit)).toEqual({
      current_space: "PASS",
      escape: "PASS",
      latest_stable: "PASS",
      macos_15_floor: "PASS",
      outside_click: "PASS",
      persisted_launch_at_login: "PASS",
      positioning_clamping: "PASS",
      rapid_interaction: "PASS",
      toggling: "PASS",
    });
    expect(
      buildAutomatedPreflight(
        { ...allLocal, rapidInteraction: false },
        { ...ci, headSha: "c".repeat(40) },
        commit,
      ),
    ).toMatchObject({
      latest_stable: "FAIL",
      macos_15_floor: "FAIL",
      rapid_interaction: "FAIL",
    });
  });
});
