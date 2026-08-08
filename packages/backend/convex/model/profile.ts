import type { GenericId } from "convex/values";

import type { MutationCtx, QueryCtx } from "../_generated/server";
import { rejectAuthority } from "./authority";

const INSTALLATION_CREDENTIAL_PATTERN =
  /^[23456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz]{52}$/;
const INITIAL_ACTIVE_MAC_GENERATION = 1;

export type AuthUserReference = {
  id: string;
};

function assertInstallationCredential(installationCredential: string) {
  if (!INSTALLATION_CREDENTIAL_PATTERN.test(installationCredential)) {
    rejectAuthority();
  }
}

export async function installationCredentialDigest(
  installationCredential: string,
) {
  assertInstallationCredential(installationCredential);
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(installationCredential),
  );
  return `sha256:${Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("")}`;
}

export async function tokenmaxxerForAuthUser(
  ctx: QueryCtx | MutationCtx,
  authUser: AuthUserReference,
) {
  return ctx.db
    .query("tokenmaxxers")
    .withIndex("by_auth_subject", (q) => q.eq("authSubject", authUser.id))
    .unique();
}

export async function ensureTokenmaxxer(
  ctx: MutationCtx,
  authUser: AuthUserReference,
  displayName: string,
  publicId: string,
) {
  const existing = await tokenmaxxerForAuthUser(ctx, authUser);
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
    authSubject: authUser.id,
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

export async function claimActiveDevice(
  ctx: MutationCtx,
  tokenmaxxerId: GenericId<"tokenmaxxers">,
  installationCredential: string,
) {
  const installationCredentialDigestValue =
    await installationCredentialDigest(installationCredential);
  const tokenmaxxer = await ctx.db.get(tokenmaxxerId);
  if (!tokenmaxxer) {
    return rejectAuthority();
  }

  if (!tokenmaxxer.activeDeviceId) {
    const deviceId = await ctx.db.insert("devices", {
      createdAt: Date.now(),
      generation: INITIAL_ACTIVE_MAC_GENERATION,
      installationCredentialDigest: installationCredentialDigestValue,
      lastSeenAt: Date.now(),
      tokenmaxxerId,
    });
    await ctx.db.patch(tokenmaxxerId, { activeDeviceId: deviceId });
    const device = await ctx.db.get(deviceId);
    if (!device) return rejectAuthority();
    return device;
  }

  const device = await ctx.db.get(tokenmaxxer.activeDeviceId);
  if (
    !device ||
    device.tokenmaxxerId !== tokenmaxxerId ||
    device.revokedAt !== undefined
  ) {
    return rejectAuthority();
  }

  if (
    !Number.isSafeInteger(device.generation) ||
    device.generation < INITIAL_ACTIVE_MAC_GENERATION ||
    device.installationCredentialDigest !== installationCredentialDigestValue
  ) {
    return rejectAuthority();
  }
  await ctx.db.patch(device._id, { lastSeenAt: Date.now() });
  return device;
}

export async function requireActiveDevice(
  ctx: MutationCtx,
  authUser: AuthUserReference,
  installationCredential: string,
  activeMacGeneration: number,
) {
  if (
    !Number.isSafeInteger(activeMacGeneration) ||
    activeMacGeneration < INITIAL_ACTIVE_MAC_GENERATION
  ) {
    return rejectAuthority();
  }
  const installationCredentialDigestValue =
    await installationCredentialDigest(installationCredential);
  const tokenmaxxer = await tokenmaxxerForAuthUser(ctx, authUser);
  if (!tokenmaxxer?.activeDeviceId) {
    return rejectAuthority();
  }
  const device = await ctx.db.get(tokenmaxxer.activeDeviceId);
  if (
    !device ||
    device.tokenmaxxerId !== tokenmaxxer._id ||
    device.revokedAt !== undefined ||
    device.generation !== activeMacGeneration ||
    device.installationCredentialDigest !== installationCredentialDigestValue
  ) {
    return rejectAuthority();
  }
  return { device, tokenmaxxer };
}
