import { v } from "convex/values";

import type { Doc } from "./_generated/dataModel";
import { query, type QueryCtx } from "./_generated/server";
import { requireAuthUser } from "./auth";
import {
  doomerboard,
  type DoomerboardKey,
  type LegacyDoomerboardKey,
} from "./model/doomerboard";
import { rejectAuthority } from "./model/authority";
import { tokenmaxxerForAuthUser } from "./model/profile";
import {
  apiEquivalentCostValidator,
  assertRankingDay,
  boardKey,
  MAX_SAVED_TOKENMAXXERS,
  rankingDayAt,
  scoreScopeValidator,
  scoreWindowValidator,
  type ApiEquivalentCost,
} from "./model/values";

const doomerboardRow = v.object({
  apiEquivalentCost: apiEquivalentCostValidator,
  displayName: v.string(),
  rank: v.number(),
  tokenScore: v.number(),
  touchGrassId: v.string(),
});

const CURRENT_GLOBAL_SCAN_LIMIT = 640;
const SCAN_ROWS_PER_KEY_FORMAT = CURRENT_GLOBAL_SCAN_LIMIT / 2;
const MAX_LEGACY_COMPATIBILITY_ROWS = CURRENT_GLOBAL_SCAN_LIMIT;
const canonicalKeyBounds = {
  lower: {
    inclusive: true,
    key: [-Number.MAX_SAFE_INTEGER, ""] as DoomerboardKey,
  },
  upper: {
    inclusive: true,
    key: [0, "\uffff"] as DoomerboardKey,
  },
};
const legacyKeyBounds = {
  lower: { inclusive: true, key: 0 as LegacyDoomerboardKey },
  upper: {
    inclusive: true,
    key: Number.MAX_SAFE_INTEGER as LegacyDoomerboardKey,
  },
};

async function legacyCompatibilityItems(
  ctx: QueryCtx,
  namespace: string,
) {
  const count = await doomerboard.count(ctx, {
    bounds: legacyKeyBounds,
    namespace,
  });
  if (count > MAX_LEGACY_COMPATIBILITY_ROWS) {
    throw new Error("Legacy Doomerboard compatibility limit exceeded");
  }
  if (count === 0) return [];
  const page = await doomerboard.paginate(ctx, {
    bounds: legacyKeyBounds,
    namespace,
    order: "desc",
    pageSize: count,
  });
  if (
    !page.isDone ||
    page.page.length !== count ||
    page.page.some((item) => typeof item.key !== "number")
  ) {
    throw new Error("Legacy Doomerboard compatibility read is incomplete");
  }
  return page.page;
}

export function rankRows<
  T extends {
    apiEquivalentCost: ApiEquivalentCost | null;
    displayName: string;
    tokenScore: number;
    touchGrassId: string;
  },
>(rows: T[]) {
  const orderedRows = [...rows].sort(
    (left, right) =>
      right.tokenScore - left.tokenScore ||
      left.touchGrassId.localeCompare(right.touchGrassId),
  );
  let rank = 0;
  let previousScore: number | null = null;
  return orderedRows.map((row, index) => {
    if (row.tokenScore !== previousScore) {
      rank = index + 1;
      previousScore = row.tokenScore;
    }
    return {
      apiEquivalentCost: row.apiEquivalentCost,
      displayName: row.displayName,
      rank,
      tokenScore: row.tokenScore,
      touchGrassId: row.touchGrassId,
    };
  });
}

async function requireDoomerboardProfile(ctx: QueryCtx) {
  const authUser = await requireAuthUser(ctx);
  const tokenmaxxer = await tokenmaxxerForAuthUser(ctx, authUser);
  if (!tokenmaxxer) return rejectAuthority();
  return tokenmaxxer;
}

async function globalRows(
  ctx: QueryCtx,
  scope: Parameters<typeof boardKey>[0],
  windowDays: Parameters<typeof boardKey>[1],
  requestedLimit?: number,
  requiredComputedRankingDay?: string,
) {
  await requireDoomerboardProfile(ctx);
  const limit = Math.min(Math.max(Math.floor(requestedLimit ?? 50), 1), 100);
  const scanLimit =
    requiredComputedRankingDay === undefined
      ? limit
      : SCAN_ROWS_PER_KEY_FORMAT;
  const namespace = boardKey(scope, windowDays);
  const [canonicalPage, legacyItems] = await Promise.all([
    doomerboard.paginate(ctx, {
      bounds: canonicalKeyBounds,
      namespace,
      order: "asc",
      pageSize: scanLimit,
    }),
    legacyCompatibilityItems(ctx, namespace),
  ]);
  const candidates = await Promise.all(
    [...canonicalPage.page, ...legacyItems].map((item) => ctx.db.get(item.id)),
  );
  const rowsById = new Map<Doc<"publicUsages">["_id"], Doc<"publicUsages">>();
  for (const row of candidates) {
    if (
      row !== null &&
      (requiredComputedRankingDay === undefined ||
        rankingDayAt(row.computedAt) === requiredComputedRankingDay)
    ) {
      rowsById.set(row._id, row);
    }
  }
  return rankRows([...rowsById.values()]).slice(0, limit);
}

export const currentGlobal = query({
  args: {
    limit: v.optional(v.number()),
    rankingDay: v.string(),
    scope: v.optional(scoreScopeValidator),
    windowDays: v.optional(scoreWindowValidator),
  },
  returns: v.array(doomerboardRow),
  handler: (ctx, args) => {
    assertRankingDay(args.rankingDay);
    return globalRows(
      ctx,
      args.scope ?? "combined",
      args.windowDays ?? 1,
      args.limit,
      args.rankingDay,
    );
  },
});

export const global = query({
  args: {
    limit: v.optional(v.number()),
    scope: scoreScopeValidator,
    windowDays: scoreWindowValidator,
  },
  returns: v.array(doomerboardRow),
  handler: (ctx, args) =>
    globalRows(ctx, args.scope, args.windowDays, args.limit),
});

async function myTokenmaxxerRows(
  ctx: QueryCtx,
  scope: Parameters<typeof boardKey>[0],
  windowDays: Parameters<typeof boardKey>[1],
  requiredComputedRankingDay?: string,
) {
  const authUser = await requireAuthUser(ctx);
  const owner = await tokenmaxxerForAuthUser(ctx, authUser);
  if (!owner) return { rows: [], savedTokenmaxxerCount: 0 };
  const added = await ctx.db
    .query("addedTokenmaxxers")
    .withIndex("by_owner_id", (q) => q.eq("ownerId", owner._id))
    .take(MAX_SAVED_TOKENMAXXERS + 1);
  if (added.length > MAX_SAVED_TOKENMAXXERS) {
    throw new Error("My Tokenmaxxers limit exceeded");
  }
  const candidates = await Promise.all(
    added.map((edge) =>
      ctx.db
        .query("publicUsages")
        .withIndex("by_tokenmaxxer_id_and_scope_and_window_days", (q) =>
          q
            .eq("tokenmaxxerId", edge.addedId)
            .eq("scope", scope)
            .eq("windowDays", windowDays),
        )
        .unique(),
    ),
  );
  const rows = candidates.filter(
    (row): row is Doc<"publicUsages"> =>
      row !== null &&
      (requiredComputedRankingDay === undefined ||
        rankingDayAt(row.computedAt) === requiredComputedRankingDay),
  );
  return {
    rows: rankRows(rows),
    savedTokenmaxxerCount: added.length,
  };
}

export const currentMyTokenmaxxers = query({
  args: {
    rankingDay: v.string(),
    scope: scoreScopeValidator,
    windowDays: scoreWindowValidator,
  },
  returns: v.object({
    rows: v.array(doomerboardRow),
    savedTokenmaxxerCount: v.number(),
  }),
  handler: (ctx, args) => {
    assertRankingDay(args.rankingDay);
    return myTokenmaxxerRows(
      ctx,
      args.scope,
      args.windowDays,
      args.rankingDay,
    );
  },
});

export const myTokenmaxxers = query({
  args: {
    scope: scoreScopeValidator,
    windowDays: scoreWindowValidator,
  },
  returns: v.array(doomerboardRow),
  handler: async (ctx, args) =>
    (await myTokenmaxxerRows(ctx, args.scope, args.windowDays)).rows,
});
