/// <reference types="vite/client" />

import doomerboardIndexTest from "@convex-dev/aggregate/test";
import rateLimiterTest from "@convex-dev/rate-limiter/test";
import { convexTest } from "convex-test";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { rankRows } from "./doomerboards";
import { doomerboard, doomerboardKey } from "./model/doomerboard";
import { claimActiveDevice } from "./model/profile";
import { calculateScore, recomputeScores } from "./model/scores";
import { applyProviderSettings, applyUsageSnapshots } from "./model/sync";
import { boardKey, SCOPES, WINDOWS } from "./model/values";
import schema from "./schema";

const modules = import.meta.glob("./**/*.ts");
const SCORE_DAY = "2026-10-09";
const SCORE_NOW = new Date(`${SCORE_DAY}T12:00:00Z`);

function scoreBackend() {
  const t = convexTest(schema, modules);
  doomerboardIndexTest.register(t, "doomerboard");
  rateLimiterTest.register(t);
  return t;
}

async function seedScores(t: ReturnType<typeof scoreBackend>) {
  return t.run(async (ctx) => {
    const tokenmaxxerId = await ctx.db.insert("tokenmaxxers", {
      activeAuthSessionId: null,
      authSessionGeneration: 1,
      authSubject: "synthetic-score-owner",
      createdAt: Date.now(),
      displayName: "Synthetic",
      publicId: "TG-ABCDEF",
    });
    const dailyId = await ctx.db.insert("userDailyUsage", {
      apiEquivalentCost: {
        coveragePercent: null,
        micros: 1_000,
        pricingBasis: "openai-api-2026-08-09-v3",
        quality: "local-only",
      },
      observedTokens: 100,
      provider: "codex",
      rankingDay: SCORE_DAY,
      tokenmaxxerId,
      updatedAt: Date.now(),
    });
    await recomputeScores(ctx, tokenmaxxerId, SCORE_DAY);
    return { dailyId, tokenmaxxerId };
  });
}

describe("materialized score updates", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(SCORE_NOW);
  });
  afterEach(() => vi.useRealTimers());

  test("a cost-only recompute updates projections without rewriting rank entries", async () => {
    const t = scoreBackend();
    const owner = await seedScores(t);
    await t.run(async (ctx) => {
      const daily = await ctx.db.get(owner.dailyId);
      if (!daily?.apiEquivalentCost) throw new Error("Synthetic cost missing");
      await ctx.db.patch(owner.dailyId, {
        apiEquivalentCost: { ...daily.apiEquivalentCost, micros: 2_000 },
      });
    });
    const metrics = await t.run(async (ctx) => {
      await recomputeScores(ctx, owner.tokenmaxxerId, SCORE_DAY);
      return ctx.meta.getTransactionMetrics();
    });
    // Nine public projections need fresh cost/time values. Their rank keys stay unchanged.
    expect(metrics.documentsWritten.used).toBe(9);
    expect(metrics.documentsRead.used).toBeLessThanOrEqual(50);
    await t.run(async (ctx) => {
      const rows = await ctx.db
        .query("publicUsages")
        .withIndex("by_tokenmaxxer_id", (q) => q.eq("tokenmaxxerId", owner.tokenmaxxerId))
        .take(20);
      expect(rows).toHaveLength(9);
      for (const scope of SCOPES) {
        for (const windowDays of WINDOWS) {
          const row = rows.find(
            (value) => value.scope === scope && value.windowDays === windowDays,
          )!;
          expect(row.tokenScore).toBe(scope === "claude" ? 0 : 100);
          expect(row.apiEquivalentCost?.micros ?? null).toBe(scope === "claude" ? null : 2_000);
          expect(await doomerboard.count(ctx, { namespace: boardKey(scope, windowDays) })).toBe(1);
          expect(
            await doomerboard.at(ctx, 0, { namespace: boardKey(scope, windowDays) }),
          ).toMatchObject({
            id: row._id,
            key: doomerboardKey(row.tokenScore, row.touchGrassId),
          });
        }
      }
    });
  });

  test("an unchanged score repairs a missing aggregate entry", async () => {
    const t = scoreBackend();
    const owner = await seedScores(t);
    await t.run(async (ctx) => {
      const row = await ctx.db
        .query("publicUsages")
        .withIndex("by_tokenmaxxer_id_and_scope_and_window_days", (q) =>
          q.eq("tokenmaxxerId", owner.tokenmaxxerId).eq("scope", "combined").eq("windowDays", 1),
        )
        .unique();
      if (!row) throw new Error("Synthetic projection missing");
      await doomerboard.delete(ctx, {
        id: row._id,
        key: doomerboardKey(row.tokenScore, row.touchGrassId),
        namespace: row.boardKey,
      });
      expect(await doomerboard.count(ctx, { namespace: row.boardKey })).toBe(0);
      await recomputeScores(ctx, owner.tokenmaxxerId, SCORE_DAY);
      expect(await doomerboard.count(ctx, { namespace: row.boardKey })).toBe(1);
      expect(await doomerboard.at(ctx, 0, { namespace: row.boardKey })).toMatchObject({
        id: row._id,
      });
    });
  });

  test("routine usage and provider settings leave stable Active Mac authority unchanged", async () => {
    const t = scoreBackend();
    const owner = await seedScores(t);
    const credential = "A".repeat(52);
    const before = await t.run(async (ctx) => {
      const device = await claimActiveDevice(ctx, owner.tokenmaxxerId, credential);
      await ctx.db.patch(device._id, { usageBackfillCompletedAt: Date.now() });
      return (await ctx.db.get(device._id))!;
    });
    vi.advanceTimersByTime(10_000);
    await t.run(async (ctx) => {
      expect(
        await applyUsageSnapshots(
          ctx,
          { id: "synthetic-score-owner" },
          credential,
          1,
          [
            {
              apiEquivalentCost: null,
              correctionReason: null,
              correctionRevision: null,
              coverage: "complete",
              evidenceBasis: "locally-derived",
              observedAt: Date.now(),
              observedTokens: 100,
              provider: "codex",
              rankingDay: SCORE_DAY,
              revision: 1,
            },
          ],
          null,
        ),
      ).toMatchObject([{ outcome: "committed" }]);
      expect(await ctx.db.get(before._id)).toEqual(before);
      expect((await ctx.db.get(owner.tokenmaxxerId))?.lastSyncedAt).toBe(Date.now());
    });
    vi.advanceTimersByTime(10_000);
    await t.run(async (ctx) => {
      expect(
        await applyProviderSettings(ctx, { id: "synthetic-score-owner" }, credential, 1, 1, [
          "codex",
        ]),
      ).toEqual({ outcome: "committed", revision: 1 });
      expect(await ctx.db.get(before._id)).toEqual(before);
      expect((await ctx.db.get(owner.tokenmaxxerId))?.lastSyncedAt).toBe(Date.now());
    });
  });

  test("recompute moves an existing entry from its old namespace and ordering key", async () => {
    const t = scoreBackend();
    const owner = await seedScores(t);
    await t.run(async (ctx) => {
      const row = await ctx.db
        .query("publicUsages")
        .withIndex("by_tokenmaxxer_id_and_scope_and_window_days", (q) =>
          q.eq("tokenmaxxerId", owner.tokenmaxxerId).eq("scope", "combined").eq("windowDays", 1),
        )
        .unique();
      if (!row) throw new Error("Synthetic projection missing");
      const oldNamespace = "tokens-v0:combined:1d";
      await doomerboard.replaceOrInsert(
        ctx,
        {
          id: row._id,
          key: doomerboardKey(row.tokenScore, row.touchGrassId),
          namespace: row.boardKey,
        },
        { key: doomerboardKey(50, row.touchGrassId), namespace: oldNamespace },
      );
      await ctx.db.patch(row._id, { boardKey: oldNamespace, tokenScore: 50 });
      await recomputeScores(ctx, owner.tokenmaxxerId, SCORE_DAY);
      expect(await doomerboard.count(ctx, { namespace: oldNamespace })).toBe(0);
      expect(await doomerboard.at(ctx, 0, { namespace: boardKey("combined", 1) })).toMatchObject({
        id: row._id,
        key: doomerboardKey(100, row.touchGrassId),
      });
    });
  });
});

describe("cost and ranking independence", () => {
  test("a cost-only reprice does not change Token Score or rank", () => {
    const baseRow = {
      apiEquivalentCost: {
        coveragePercent: null,
        micros: 100_000,
        pricingBasis: "openai-api-2026-08-09-v3",
        quality: "local-only" as const,
      },
      observedTokens: 100,
      provider: "codex",
      rankingDay: "2026-08-06",
    };
    const before = calculateScore([baseRow], "codex", 1, "2026-08-06");
    const after = calculateScore(
      [
        {
          ...baseRow,
          apiEquivalentCost: {
            ...baseRow.apiEquivalentCost,
            micros: 250_000,
          },
        },
      ],
      "codex",
      1,
      "2026-08-06",
    );

    const beforeCost = before.apiEquivalentCost;
    const afterCost = after.apiEquivalentCost;
    if (beforeCost === null || afterCost === null) {
      throw new Error("priced rows must produce an API-equivalent cost");
    }
    expect(afterCost.micros).not.toBe(beforeCost.micros);
    expect(after.tokenScore).toBe(before.tokenScore);

    const board = (micros: number) =>
      rankRows([
        {
          apiEquivalentCost: { ...beforeCost, micros: 300_000 },
          displayName: "Higher",
          tokenScore: 200,
          touchGrassId: "TG-HIGHER",
        },
        {
          apiEquivalentCost: { ...beforeCost, micros },
          displayName: "Repriced",
          tokenScore: after.tokenScore,
          touchGrassId: "TG-REPRICED",
        },
        {
          apiEquivalentCost: { ...beforeCost, micros: 50_000 },
          displayName: "Lower",
          tokenScore: 50,
          touchGrassId: "TG-LOWER",
        },
      ]).map(({ rank, tokenScore, touchGrassId }) => ({
        rank,
        tokenScore,
        touchGrassId,
      }));

    expect(board(afterCost.micros)).toEqual(board(beforeCost.micros));
  });

  test("modeled cost metadata survives combined score and board projection", () => {
    const score = calculateScore(
      [
        {
          apiEquivalentCost: {
            coveragePercent: null,
            micros: 1_000_000,
            pricingBasis: "openai-api-2026-08-09-v3",
            quality: "reconciled" as const,
          },
          observedTokens: 100,
          provider: "codex",
          rankingDay: "2026-08-06",
        },
        {
          apiEquivalentCost: {
            coveragePercent: 50,
            micros: 2_000_000,
            pricingBasis: "anthropic-standard-2026-08-07-v1",
            quality: "modeled" as const,
          },
          observedTokens: 300,
          provider: "claude",
          rankingDay: "2026-08-06",
        },
        {
          apiEquivalentCost: null,
          observedTokens: 100,
          provider: "codex",
          rankingDay: "2026-08-06",
        },
      ],
      "combined",
      1,
      "2026-08-06",
    );

    expect(score).toEqual({
      apiEquivalentCost: {
        coveragePercent: 50,
        micros: 3_000_000,
        pricingBasis: "anthropic-standard-2026-08-07-v1 + openai-api-2026-08-09-v3",
        quality: "modeled",
      },
      tokenScore: 500,
    });
    const projectedCost = score.apiEquivalentCost;
    if (!projectedCost) {
      throw new Error("priced rows must keep the complete cost object");
    }
    expect(
      rankRows([
        {
          apiEquivalentCost: projectedCost,
          displayName: "Modeled",
          tokenScore: score.tokenScore,
          touchGrassId: "TG-MODELED",
        },
      ]),
    ).toEqual([
      {
        apiEquivalentCost: score.apiEquivalentCost,
        displayName: "Modeled",
        rank: 1,
        tokenScore: 500,
        touchGrassId: "TG-MODELED",
      },
    ]);
  });

  test("zero-token cost does not price unpriced positive usage", () => {
    const score = calculateScore(
      [
        {
          apiEquivalentCost: null,
          observedTokens: 100,
          provider: "codex",
          rankingDay: "2026-08-06",
        },
        {
          apiEquivalentCost: {
            coveragePercent: null,
            micros: 0,
            pricingBasis: "anthropic-standard-2026-08-07-v1",
            quality: "reconciled" as const,
          },
          observedTokens: 0,
          provider: "claude",
          rankingDay: "2026-08-06",
        },
      ],
      "combined",
      1,
      "2026-08-06",
    );

    expect(score).toEqual({
      apiEquivalentCost: null,
      tokenScore: 100,
    });
  });
});
