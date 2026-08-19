import { v } from "convex/values";

import { mutation, query } from "./_generated/server";
import { requireAuthUser } from "./auth";
import { rejectAuthority } from "./model/authority";
import {
  claimActiveDevice,
  ensureTokenmaxxer,
  tokenmaxxerForAuthUser,
} from "./model/profile";
import { MAX_SAVED_TOKENMAXXERS } from "./model/values";

const publicTokenmaxxer = v.object({
  displayName: v.string(),
  touchGrassId: v.string(),
});
const ensuredTokenmaxxer = publicTokenmaxxer.extend({
  activeMacActivatedAt: v.number(),
  activeMacGeneration: v.number(),
});

function cleanDisplayName(displayName: string) {
  const cleaned = displayName.trim();
  if (cleaned.length < 1 || cleaned.length > 40) {
    throw new Error("displayName must contain 1-40 characters");
  }
  return cleaned;
}

function touchGrassIdForAuthUser(user: { username?: unknown }) {
  if (
    typeof user.username !== "string" ||
    !/^TG-[A-HJ-NP-Z2-9]{6}$/.test(user.username)
  ) {
    return rejectAuthority();
  }
  return user.username;
}

export const ensureProfile = mutation({
  args: {
    displayName: v.string(),
    expectedTouchGrassId: v.string(),
    installationCredential: v.string(),
  },
  returns: ensuredTokenmaxxer,
  handler: async (ctx, args) => {
    const authUser = await requireAuthUser(ctx);
    const touchGrassId = touchGrassIdForAuthUser(authUser);
    if (args.expectedTouchGrassId !== touchGrassId) {
      return rejectAuthority();
    }
    const tokenmaxxer = await ensureTokenmaxxer(
      ctx,
      authUser,
      cleanDisplayName(args.displayName),
      touchGrassId,
    );
    const activeDevice = await claimActiveDevice(
      ctx,
      tokenmaxxer._id,
      args.installationCredential,
    );
    if (
      !Number.isSafeInteger(activeDevice.createdAt) ||
      activeDevice.createdAt < 0
    ) {
      return rejectAuthority();
    }
    return {
      activeMacActivatedAt: activeDevice.createdAt,
      activeMacGeneration: activeDevice.generation,
      displayName: tokenmaxxer.displayName,
      touchGrassId: tokenmaxxer.publicId,
    };
  },
});

export const updateDisplayName = mutation({
  args: { displayName: v.string() },
  returns: publicTokenmaxxer,
  handler: async (ctx, args) => {
    const authUser = await requireAuthUser(ctx);
    const tokenmaxxer = await tokenmaxxerForAuthUser(ctx, authUser);
    if (!tokenmaxxer) {
      throw new Error("TouchGrass Profile not found");
    }
    const displayName = cleanDisplayName(args.displayName);
    const publicUsages = await ctx.db
      .query("publicUsages")
      .withIndex("by_tokenmaxxer_id", (q) =>
        q.eq("tokenmaxxerId", tokenmaxxer._id),
      )
      .take(20);
    await ctx.db.patch(tokenmaxxer._id, { displayName });
    for (const publicUsage of publicUsages) {
      await ctx.db.patch(publicUsage._id, { displayName });
    }
    return { displayName, touchGrassId: tokenmaxxer.publicId };
  },
});

export const findByTouchGrassId = query({
  args: { touchGrassId: v.string() },
  returns: v.union(publicTokenmaxxer, v.null()),
  handler: async (ctx, args) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", args.touchGrassId))
      .unique();
    return tokenmaxxer
      ? { displayName: tokenmaxxer.displayName, touchGrassId: tokenmaxxer.publicId }
      : null;
  },
});

export const addToMyTokenmaxxers = mutation({
  args: { touchGrassId: v.string() },
  returns: v.null(),
  handler: async (ctx, args) => {
    const authUser = await requireAuthUser(ctx);
    const owner = await tokenmaxxerForAuthUser(ctx, authUser);
    if (!owner) {
      throw new Error("TouchGrass Profile not found");
    }
    const added = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", args.touchGrassId))
      .unique();
    if (!added || added._id === owner._id) {
      throw new Error("Tokenmaxxer cannot be added");
    }
    const existing = await ctx.db
      .query("addedTokenmaxxers")
      .withIndex("by_owner_id_and_added_id", (q) =>
        q.eq("ownerId", owner._id).eq("addedId", added._id),
      )
      .unique();
    if (existing) return null;
    const saved = await ctx.db
      .query("addedTokenmaxxers")
      .withIndex("by_owner_id", (q) => q.eq("ownerId", owner._id))
      .take(MAX_SAVED_TOKENMAXXERS);
    if (saved.length >= MAX_SAVED_TOKENMAXXERS) {
      throw new Error("My Tokenmaxxers limit reached");
    }
    await ctx.db.insert("addedTokenmaxxers", {
      addedId: added._id,
      createdAt: Date.now(),
      ownerId: owner._id,
    });
    return null;
  },
});

export const removeFromMyTokenmaxxers = mutation({
  args: { touchGrassId: v.string() },
  returns: v.null(),
  handler: async (ctx, args) => {
    const authUser = await requireAuthUser(ctx);
    const owner = await tokenmaxxerForAuthUser(ctx, authUser);
    if (!owner) {
      throw new Error("TouchGrass Profile not found");
    }
    const added = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", args.touchGrassId))
      .unique();
    if (!added) {
      return null;
    }
    const edge = await ctx.db
      .query("addedTokenmaxxers")
      .withIndex("by_owner_id_and_added_id", (q) =>
        q.eq("ownerId", owner._id).eq("addedId", added._id),
      )
      .unique();
    if (edge) {
      await ctx.db.delete(edge._id);
    }
    return null;
  },
});
