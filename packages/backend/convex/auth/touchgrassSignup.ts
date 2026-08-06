import type { BetterAuthPlugin } from "better-auth";
import {
  APIError,
  createAuthEndpoint,
  createAuthMiddleware,
  isAPIError,
} from "better-auth/api";
import { v } from "convex/values";

import { internal } from "../_generated/api";
import type { Id } from "../_generated/dataModel";
import { internalMutation } from "../_generated/server";

const PREPARATION_LIFETIME_MS = 120_000;
const PUBLIC_ID_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const PUBLIC_ID_PATTERN = /^TG-[A-HJ-NP-Z2-9]{6}$/;
const PROOF_HEADER = "x-touchgrass-signup-proof";

type PreparationPayload = {
  expiresAt: number;
  nonce: string;
  touchGrassId: string;
  version: 1;
};

type FailedCredentialKeys = {
  ipKey: string;
  touchGrassIdKey: string;
};

export type TouchGrassPolicyPort = {
  consumeSignupProof: (args: {
    nonceDigest: string;
    touchGrassId: string;
  }) => Promise<boolean>;
  finalizeCredentialAttempt: (args: {
    outcome: "failure" | "success";
    reservationId: Id<"recoveryKeyAttemptReservations">;
  }) => Promise<boolean>;
  issueSignupProof: (args: {
    expiresAt: number;
    nonceDigest: string;
    touchGrassId: string;
  }) => Promise<void>;
  limitProfilePreparation: (args: { ipKey: string }) => Promise<boolean>;
  requestIpAddress: () => Promise<string | null>;
  reserveCredentialAttempt: (
    keys: FailedCredentialKeys,
  ) => Promise<Id<"recoveryKeyAttemptReservations"> | null>;
};

function bytesToBase64Url(bytes: Uint8Array) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary)
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");
}

function base64UrlToBytes(value: string) {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) return null;
  const normalized = value.replaceAll("-", "+").replaceAll("_", "/");
  const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "=");
  try {
    return Uint8Array.from(atob(padded), (character) =>
      character.charCodeAt(0),
    );
  } catch {
    return null;
  }
}

function randomBytes(length: number) {
  return crypto.getRandomValues(new Uint8Array(length));
}

function createTouchGrassId() {
  const suffix = [...randomBytes(6)]
    .map((byte) => PUBLIC_ID_ALPHABET[byte % PUBLIC_ID_ALPHABET.length])
    .join("");
  return `TG-${suffix}`;
}

function syntheticEmail(touchGrassId: string) {
  return `${touchGrassId.toLowerCase()}@profile.touchgrass.invalid`;
}

async function hmacKey(secret: string) {
  return crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { hash: "SHA-256", name: "HMAC" },
    false,
    ["sign", "verify"],
  );
}

async function sha256Digest(value: string) {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(value),
  );
  return bytesToBase64Url(new Uint8Array(digest));
}

async function opaqueLimitKey(
  secret: string,
  scope: "ip" | "profile-preparation-ip" | "touchgrass-id",
  value: string,
) {
  const digest = await crypto.subtle.sign(
    "HMAC",
    await hmacKey(secret),
    new TextEncoder().encode(`${scope}\0${value}`),
  );
  return bytesToBase64Url(new Uint8Array(digest));
}

async function signPreparation(secret: string, payload: PreparationPayload) {
  const encodedPayload = bytesToBase64Url(
    new TextEncoder().encode(JSON.stringify(payload)),
  );
  const signature = await crypto.subtle.sign(
    "HMAC",
    await hmacKey(secret),
    new TextEncoder().encode(encodedPayload),
  );
  return `${encodedPayload}.${bytesToBase64Url(new Uint8Array(signature))}`;
}

async function verifyPreparation(
  secret: string,
  proof: string,
  now: number,
) {
  if (proof.length > 1_024) return null;
  const [encodedPayload, encodedSignature, extra] = proof.split(".");
  if (!encodedPayload || !encodedSignature || extra !== undefined) return null;
  const payloadBytes = base64UrlToBytes(encodedPayload);
  const signature = base64UrlToBytes(encodedSignature);
  if (!payloadBytes || !signature) return null;
  const validSignature = await crypto.subtle.verify(
    "HMAC",
    await hmacKey(secret),
    signature,
    new TextEncoder().encode(encodedPayload),
  );
  if (!validSignature) return null;

  try {
    const value = JSON.parse(new TextDecoder().decode(payloadBytes)) as unknown;
    if (!value || typeof value !== "object") return null;
    const payload = value as Partial<PreparationPayload>;
    if (
      payload.version !== 1 ||
      typeof payload.expiresAt !== "number" ||
      !Number.isSafeInteger(payload.expiresAt) ||
      payload.expiresAt <= now ||
      typeof payload.nonce !== "string" ||
      payload.nonce.length < 20 ||
      typeof payload.touchGrassId !== "string" ||
      !PUBLIC_ID_PATTERN.test(payload.touchGrassId)
    ) {
      return null;
    }
    return payload as PreparationPayload;
  } catch {
    return null;
  }
}

function signupProofFromHeaders(context: {
  headers?: Headers | undefined;
  request?: { headers: Headers } | undefined;
}) {
  return (
    context.request?.headers.get(PROOF_HEADER) ??
    context.headers?.get(PROOF_HEADER) ??
    null
  );
}

function rejectRateLimitedCredential(): never {
  throw new APIError("TOO_MANY_REQUESTS", {
    message: "Too many requests. Please try again later.",
  });
}

async function requestIpAddress(policy: TouchGrassPolicyPort) {
  const ipAddress = await policy.requestIpAddress().catch(() => null);
  if (!ipAddress) rejectRateLimitedCredential();
  return ipAddress;
}

function reservationIdFromHookContext(
  value: unknown,
): Id<"recoveryKeyAttemptReservations"> | null {
  if (!value || typeof value !== "object") return null;
  const reservationId = Reflect.get(
    value,
    "touchGrassRecoveryReservationId",
  );
  return typeof reservationId === "string"
    ? (reservationId as Id<"recoveryKeyAttemptReservations">)
    : null;
}

function touchGrassIdLimitInput(value: unknown) {
  if (typeof value !== "string" || value.length !== 9) {
    return "invalid-touchgrass-id";
  }
  const normalized = value.toUpperCase();
  return PUBLIC_ID_PATTERN.test(normalized)
    ? normalized
    : "invalid-touchgrass-id";
}

async function failedCredentialKeys(
  secret: string,
  ipAddress: string,
  touchGrassId: unknown,
): Promise<FailedCredentialKeys> {
  const touchGrassIdInput = touchGrassIdLimitInput(touchGrassId);
  const [ipKey, touchGrassIdKey] = await Promise.all([
    opaqueLimitKey(secret, "ip", ipAddress),
    opaqueLimitKey(secret, "touchgrass-id", touchGrassIdInput),
  ]);
  return { ipKey, touchGrassIdKey };
}

export function touchGrassSignup(policy: TouchGrassPolicyPort): BetterAuthPlugin {
  return {
    id: "touchgrass-signup",
    endpoints: {
      prepareTouchGrassSignup: createAuthEndpoint(
        "/touchgrass/prepare",
        { method: "POST" },
        async (ctx) => {
          const ipAddress = await requestIpAddress(policy);
          const ipKey = await opaqueLimitKey(
            ctx.context.secret,
            "profile-preparation-ip",
            ipAddress,
          );
          const allowed = await policy
            .limitProfilePreparation({ ipKey })
            .catch(() => false);
          if (!allowed) rejectRateLimitedCredential();

          const touchGrassId = createTouchGrassId();
          const payload: PreparationPayload = {
            expiresAt: Date.now() + PREPARATION_LIFETIME_MS,
            nonce: bytesToBase64Url(randomBytes(16)),
            touchGrassId,
            version: 1,
          };
          const signupProof = await signPreparation(ctx.context.secret, payload);
          await policy.issueSignupProof({
            expiresAt: payload.expiresAt,
            nonceDigest: await sha256Digest(payload.nonce),
            touchGrassId,
          });
          return ctx.json({
            expiresAt: payload.expiresAt,
            signupProof,
            touchGrassId,
          });
        },
      ),
    },
    hooks: {
      before: [
        {
          matcher: (context) => context.path === "/sign-up/email",
          handler: createAuthMiddleware(async (ctx) => {
            const proof = signupProofFromHeaders(ctx);
            const preparation = proof
              ? await verifyPreparation(ctx.context.secret, proof, Date.now())
              : null;
            if (
              !preparation ||
              ctx.body.username !== preparation.touchGrassId ||
              ctx.body.email !== syntheticEmail(preparation.touchGrassId)
            ) {
              throw new APIError("FORBIDDEN", {
                message: "Signup preparation is invalid or expired",
              });
            }
            const consumed = await policy.consumeSignupProof({
              nonceDigest: await sha256Digest(preparation.nonce),
              touchGrassId: preparation.touchGrassId,
            });
            if (!consumed) {
              throw new APIError("FORBIDDEN", {
                message: "Signup preparation is invalid or expired",
              });
            }
          }),
        },
        {
          matcher: (context) => context.path === "/sign-in/username",
          handler: createAuthMiddleware(async (ctx) => {
            const ipAddress = await requestIpAddress(policy);
            const keys = await failedCredentialKeys(
              ctx.context.secret,
              ipAddress,
              ctx.body.username,
            );
            const reservationId = await policy
              .reserveCredentialAttempt(keys)
              .catch(() => null);
            if (!reservationId) rejectRateLimitedCredential();
            return {
              context: { touchGrassRecoveryReservationId: reservationId },
            };
          }),
        },
      ],
      after: [
        {
          matcher: (context) => context.path === "/sign-in/username",
          handler: createAuthMiddleware(async (ctx) => {
            const reservationId = reservationIdFromHookContext(ctx);
            if (!reservationId) rejectRateLimitedCredential();
            const completed = await policy
              .finalizeCredentialAttempt({
                outcome: isAPIError(ctx.context.returned)
                  ? "failure"
                  : "success",
                reservationId,
              })
              .catch(() => false);
            if (!completed) rejectRateLimitedCredential();
          }),
        },
      ],
    },
  };
}

export const issueSignupProof = internalMutation({
  args: {
    expiresAt: v.number(),
    nonceDigest: v.string(),
    touchGrassId: v.string(),
  },
  returns: v.null(),
  handler: async (ctx, args): Promise<null> => {
    if (!Number.isSafeInteger(args.expiresAt) || args.expiresAt <= Date.now()) {
      throw new Error("Signup proof expiry must be in the future");
    }
    const existing = await ctx.db
      .query("signupProofs")
      .withIndex("by_nonce_digest", (query) =>
        query.eq("nonceDigest", args.nonceDigest),
      )
      .unique();
    if (existing) throw new Error("Signup proof nonce collision");

    const signupProofId = await ctx.db.insert("signupProofs", args);
    await ctx.scheduler.runAt(
      args.expiresAt,
      internal.auth.touchgrassSignup.expireSignupProof,
      { signupProofId },
    );
    return null;
  },
});

export const consumeSignupProof = internalMutation({
  args: { nonceDigest: v.string(), touchGrassId: v.string() },
  returns: v.boolean(),
  handler: async (ctx, args) => {
    const signupProof = await ctx.db
      .query("signupProofs")
      .withIndex("by_nonce_digest", (query) =>
        query.eq("nonceDigest", args.nonceDigest),
      )
      .unique();
    if (!signupProof) return false;
    if (signupProof.expiresAt <= Date.now()) {
      await ctx.db.delete(signupProof._id);
      return false;
    }
    if (signupProof.touchGrassId !== args.touchGrassId) return false;

    await ctx.db.delete(signupProof._id);
    return true;
  },
});

export const expireSignupProof = internalMutation({
  args: { signupProofId: v.id("signupProofs") },
  returns: v.null(),
  handler: async (ctx, args): Promise<null> => {
    const signupProof = await ctx.db.get(args.signupProofId);
    if (!signupProof) return null;
    if (signupProof.expiresAt > Date.now()) {
      await ctx.scheduler.runAt(
        signupProof.expiresAt,
        internal.auth.touchgrassSignup.expireSignupProof,
        args,
      );
      return null;
    }
    await ctx.db.delete(signupProof._id);
    return null;
  },
});
