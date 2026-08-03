import type { GenericId } from "convex/values";

import type { MutationCtx, QueryCtx } from "../_generated/server";

const PUBLIC_ID_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

function randomPublicId() {
  const bytes = crypto.getRandomValues(new Uint8Array(6));
  const suffix = [...bytes]
    .map((byte) => PUBLIC_ID_ALPHABET[byte % PUBLIC_ID_ALPHABET.length])
    .join("");
  return `TG-${suffix}`;
}

export async function tokenmaxxerForSubject(
  ctx: QueryCtx | MutationCtx,
  authSubject: string,
) {
  return ctx.db
    .query("tokenmaxxers")
    .withIndex("by_auth_subject", (q) => q.eq("authSubject", authSubject))
    .unique();
}

export async function ensureTokenmaxxer(
  ctx: MutationCtx,
  authSubject: string,
  displayName: string,
) {
  const existing = await tokenmaxxerForSubject(ctx, authSubject);
  if (existing) {
    return existing;
  }

  for (let attempt = 0; attempt < 5; attempt += 1) {
    const publicId = randomPublicId();
    const collision = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", publicId))
      .unique();
    if (!collision) {
      const tokenmaxxerId = await ctx.db.insert("tokenmaxxers", {
        authSubject,
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
  }

  throw new Error("could not allocate a TouchGrass ID");
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
