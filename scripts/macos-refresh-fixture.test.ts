import { createHash } from "node:crypto";

import { describe, expect, test, vi } from "vitest";
import { sanitizedDesktopStateSchema } from "../packages/contracts/src/native.generated";

import {
  REFRESH_FIXTURE_BYTES,
  REFRESH_FIXTURE_SHA256,
  REFRESH_FIXTURE_VERSION,
  createMacosRefreshFixtureBinding,
  generateMacosRefreshFixtureBytes,
  runMacosRefreshFixtureCli,
  type MacosRefreshFixture,
} from "./macos-refresh-fixture";

function parseFixture(bytes = generateMacosRefreshFixtureBytes()) {
  return JSON.parse(Buffer.from(bytes).toString("utf8")) as MacosRefreshFixture;
}

describe("macOS refresh fixture", () => {
  test("generates deterministic synthetic product maxima", () => {
    const first = generateMacosRefreshFixtureBytes();
    const second = generateMacosRefreshFixtureBytes();
    const fixture = parseFixture(first);

    expect(first).toEqual(second);
    expect(fixture.version).toBe(REFRESH_FIXTURE_VERSION);
    expect(fixture.source).toBe("synthetic");
    expect(fixture.maxima).toEqual({
      global_rows: 100,
      model_cost_days_per_provider: 30,
      my_tokenmaxxers_rows: 100,
      ranking_days_per_provider: 60,
      supported_providers: 2,
    });
    expect(fixture.providers.map(({ provider }) => provider)).toEqual(["codex", "claude"]);

    for (const provider of fixture.providers) {
      expect(provider.ranking_days).toHaveLength(60);
      expect(provider.model_cost_days).toHaveLength(30);
      expect(new Set(provider.ranking_days.map(({ ranking_day }) => ranking_day)).size).toBe(60);
      expect(provider.ranking_days.map(({ ranking_day }) => ranking_day)).toEqual(
        Array.from({ length: 60 }, (_, offset) => {
          const day = new Date(Date.UTC(2026, 7, 8));
          day.setUTCDate(day.getUTCDate() - offset);
          return day.toISOString().slice(0, 10);
        }),
      );
      expect(provider.model_cost_days.map(({ ranking_day }) => ranking_day)).toEqual(
        provider.ranking_days.slice(0, 30).map(({ ranking_day }) => ranking_day),
      );
    }

    expect(fixture.doomerboards.global).toHaveLength(100);
    expect(fixture.doomerboards.my_tokenmaxxers).toHaveLength(100);
    expect(fixture.panel_projection.providers).toHaveLength(2);
    expect(sanitizedDesktopStateSchema.safeParse(fixture.panel_projection).success).toBe(true);
    expect(fixture.panel_projection.providers).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ provider: "codex", presence: "detected" }),
        expect.objectContaining({ provider: "claude", presence: "detected" }),
      ]),
    );
    for (const rows of [fixture.doomerboards.global, fixture.doomerboards.my_tokenmaxxers]) {
      expect(rows.map(({ rank }) => rank)).toEqual(
        Array.from({ length: 100 }, (_, index) => index + 1),
      );
      expect(new Set(rows.map(({ touchGrassId }) => touchGrassId)).size).toBe(100);
      expect(rows.every(({ displayName }) => displayName.length === 40)).toBe(true);
      expect(rows.every(({ touchGrassId }) => /^TG-[A-HJ-NP-Z2-9]{6}$/.test(touchGrassId))).toBe(
        true,
      );
    }

    const serialized = Buffer.from(first).toString("utf8");
    expect(serialized).not.toContain("My Tokenmaxxer ");
    expect(serialized).not.toMatch(
      /credential|recovery|session|authorization|access[_-]?token|raw[_-]?provider|logs?|\/Users\/|\/private\//i,
    );
  });

  test("binds the exact fixture bytes to SHA-256", () => {
    const bytes = generateMacosRefreshFixtureBytes();

    expect(createMacosRefreshFixtureBinding(bytes)).toEqual({
      version: REFRESH_FIXTURE_VERSION,
      sha256: REFRESH_FIXTURE_SHA256,
      bytes: REFRESH_FIXTURE_BYTES,
    });
    expect(createHash("sha256").update(bytes).digest("hex")).toBe(REFRESH_FIXTURE_SHA256);
    expect(bytes.byteLength).toBe(REFRESH_FIXTURE_BYTES);
  });

  test("writes once to the one explicit CLI output file", () => {
    const write = vi.fn();

    const binding = runMacosRefreshFixtureCli(["refresh-fixture.json"], write);

    expect(write).toHaveBeenCalledOnce();
    expect(write).toHaveBeenCalledWith("refresh-fixture.json", generateMacosRefreshFixtureBytes());
    expect(binding).toEqual(createMacosRefreshFixtureBinding());
  });

  test.each([
    { argumentsList: [] },
    { argumentsList: ["first.json", "second.json"] },
    { argumentsList: [""] },
  ])("rejects missing, extra, or empty CLI arguments: %j", ({ argumentsList }) => {
    const write = vi.fn();

    expect(() => runMacosRefreshFixtureCli(argumentsList, write)).toThrow(
      "Usage: bun scripts/macos-refresh-fixture.ts <output-file>",
    );
    expect(write).not.toHaveBeenCalled();
  });
});
