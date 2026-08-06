import type { GenericId } from "convex/values";

import type { MutationCtx, QueryCtx } from "../_generated/server";

export async function tokenmaxxerForAuthUser(
  ctx: QueryCtx | MutationCtx,
  authUserId: string,
) {
  return ctx.db
    .query("tokenmaxxers")
    .withIndex("by_auth_subject", (q) => q.eq("authSubject", authUserId))
    .unique();
}

export async function ensureTokenmaxxer(
  ctx: MutationCtx,
  authUserId: string,
  displayName: string,
  publicId: string,
) {
  const existing = await tokenmaxxerForAuthUser(ctx, authUserId);
  if (existing) {
    if (existing.publicId !== publicId) {
      throw new Error("TouchGrass Profile does not match the authenticated Profile");
    }
    return existing;
  }

  const collision = await ctx.db
    .query("tokenmaxxers")
    .withIndex("by_public_id", (q) => q.eq("publicId", publicId))
    .unique();
  if (collision) {
    throw new Error("TouchGrass Profile is unavailable");
  }

  const tokenmaxxerId = await ctx.db.insert("tokenmaxxers", {
    authSubject: authUserId,
    createdAt: Date.now(),
    displayName,
    publicId,
  });
  const created = await ctx.db.get(tokenmaxxerId);
  if (!created) {
    throw new Error("failed to create Tokenmaxxer");
  }
  return created;
}

export async function resolveActiveDevice(
  ctx: MutationCtx,
  tokenmaxxerId: GenericId<"tokenmaxxers">,
  installationId: string,
) {
  const devices = await ctx.db
    .query("devices")
    .withIndex("by_tokenmaxxer_id", (q) => q.eq("tokenmaxxerId", tokenmaxxerId))
    .take(20);
  let device = devices.find((candidate) => candidate.installationId === installationId);

  if (!device) {
    const deviceId = await ctx.db.insert("devices", {
      createdAt: Date.now(),
      installationId,
      lastSeenAt: Date.now(),
      tokenmaxxerId,
    });
    device = (await ctx.db.get(deviceId)) ?? undefined;
  }
  if (!device || device.revokedAt !== undefined) {
    throw new Error("device is revoked");
  }

  const tokenmaxxer = await ctx.db.get(tokenmaxxerId);
  if (!tokenmaxxer) {
    throw new Error("Tokenmaxxer no longer exists");
  }
  if (tokenmaxxer.activeDeviceId && tokenmaxxer.activeDeviceId !== device._id) {
    throw new Error("another Mac currently owns synchronization authority");
  }
  if (!tokenmaxxer.activeDeviceId) {
    await ctx.db.patch(tokenmaxxerId, { activeDeviceId: device._id });
  }
  await ctx.db.patch(device._id, { lastSeenAt: Date.now() });

  return device;
}
