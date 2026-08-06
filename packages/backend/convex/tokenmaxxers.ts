import { v } from "convex/values";

import { mutation, query } from "./_generated/server";
import { requireAuthUser } from "./auth";
import {
  ensureTokenmaxxer,
  tokenmaxxerForAuthUser,
} from "./model/profile";

const publicTokenmaxxer = v.object({
  displayName: v.string(),
  touchGrassId: v.string(),
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
    throw new Error("authenticated Profile has no TouchGrass ID");
  }
  return user.username;
}

export const ensureProfile = mutation({
  args: { displayName: v.string() },
  returns: publicTokenmaxxer,
  handler: async (ctx, args) => {
    const authUser = await requireAuthUser(ctx);
    const tokenmaxxer = await ensureTokenmaxxer(
      ctx,
      authUser.id,
      cleanDisplayName(args.displayName),
      touchGrassIdForAuthUser(authUser),
    );
    return {
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
    const tokenmaxxer = await tokenmaxxerForAuthUser(ctx, authUser.id);
    if (!tokenmaxxer) {
      throw new Error("TouchGrass Profile not found");
    }
    const displayName = cleanDisplayName(args.displayName);
    await ctx.db.patch(tokenmaxxer._id, { displayName });
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
    const owner = await tokenmaxxerForAuthUser(ctx, authUser.id);
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
      .withIndex("by_owner_id", (q) => q.eq("ownerId", owner._id))
      .take(500)
      .then((rows) => rows.find((row) => row.addedId === added._id));
    if (!existing) {
      await ctx.db.insert("addedTokenmaxxers", {
        addedId: added._id,
        createdAt: Date.now(),
        ownerId: owner._id,
      });
    }
    return null;
  },
});

export const removeFromMyTokenmaxxers = mutation({
  args: { touchGrassId: v.string() },
  returns: v.null(),
  handler: async (ctx, args) => {
    const authUser = await requireAuthUser(ctx);
    const owner = await tokenmaxxerForAuthUser(ctx, authUser.id);
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
      .withIndex("by_owner_id", (q) => q.eq("ownerId", owner._id))
      .take(500)
      .then((rows) => rows.find((row) => row.addedId === added._id));
    if (edge) {
      await ctx.db.delete(edge._id);
    }
    return null;
  },
});
