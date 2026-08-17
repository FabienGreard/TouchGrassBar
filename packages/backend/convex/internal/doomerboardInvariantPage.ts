import {
  paginationOptsValidator,
  paginationResultValidator,
} from "convex/server";
import { v } from "convex/values";

import { internalMutation, internalQuery } from "../_generated/server";
import {
  doomerboard,
  doomerboardKey,
  isStoredDoomerboardKey,
  storedDoomerboardKeyValidator,
} from "../model/doomerboard";
import { boardKey } from "../model/values";

const publicScoreValidator = v.object({
  boardKey: v.string(),
  id: v.id("publicUsages"),
  tokenScore: v.number(),
  touchGrassId: v.string(),
});

export const publicScores = internalQuery({
  args: { paginationOpts: paginationOptsValidator },
  returns: paginationResultValidator(publicScoreValidator),
  handler: async (ctx, args) => {
    const result = await ctx.db
      .query("publicUsages")
      .paginate(args.paginationOpts);
    return {
      ...result,
      page: result.page.map((row) => ({
        boardKey: row.boardKey,
        id: row._id,
        tokenScore: row.tokenScore,
        touchGrassId: row.touchGrassId,
      })),
    };
  },
});

export const repairEntry = internalMutation({
  args: {
    id: v.id("publicUsages"),
    namespace: v.string(),
    observedKey: v.union(storedDoomerboardKeyValidator, v.null()),
  },
  returns: v.null(),
  handler: async (ctx, args) => {
    if (args.observedKey !== null) {
      if (!isStoredDoomerboardKey(args.observedKey)) {
        throw new Error("Invalid stored Doomerboard key");
      }
      await doomerboard.deleteIfExists(ctx, {
        id: args.id,
        key: args.observedKey,
        namespace: args.namespace,
      });
    }

    const publicScore = await ctx.db.get(args.id);
    if (!publicScore) return null;
    const expectedNamespace = boardKey(
      publicScore.scope,
      publicScore.windowDays,
    );
    if (publicScore.boardKey !== expectedNamespace) {
      await ctx.db.patch(publicScore._id, {
        boardKey: expectedNamespace,
      });
    }
    await doomerboard.insertIfDoesNotExist(ctx, {
      id: publicScore._id,
      key: doomerboardKey(
        publicScore.tokenScore,
        publicScore.touchGrassId,
      ),
      namespace: expectedNamespace,
    });
    return null;
  },
});
