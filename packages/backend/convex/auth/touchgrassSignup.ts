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
const ATTEMPT_ID_PATTERN =
  /^[23456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz]{32}$/;
const INSTALLATION_CREDENTIAL_PATTERN =
  /^[23456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz]{52}$/;
const PUBLIC_ID_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const PUBLIC_ID_PATTERN = /^TG-[A-HJ-NP-Z2-9]{6}$/;
const PROOF_HEADER = "x-touchgrass-signup-proof";
const RECOVERY_KEY_PATTERN =
  /^[23456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz]{48}$/;

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

type RecoveryProofPayload = {
  attemptId: string;
  expectedGeneration: number;
  expiresAt: number;
  touchGrassId: string;
  version: 1;
};

type RecoveryCommitResult = {
  activeMacActivatedAt: number;
  activeMacGeneration: number;
  displayName: string;
  touchGrassId: string;
};

export type TouchGrassPolicyPort = {
  claimRecoveryAttempt: (args: {
    attemptDigest: string;
    authSubject: string;
  }) => Promise<boolean>;
  commitRecoveryAttempt: (args: {
    attemptDigest: string;
    authSubject: string;
    installationCredential: string;
  }) => Promise<RecoveryCommitResult | null>;
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
  prepareRecoveryAttempt: (args: {
    attemptDigest: string;
    authSubject: string;
    touchGrassId: string;
  }) => Promise<{ expectedGeneration: number; expiresAt: number } | null>;
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

async function signRecoveryProof(
  secret: string,
  payload: RecoveryProofPayload,
) {
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

async function verifyRecoveryProof(
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
    const payload = value as Partial<RecoveryProofPayload>;
    if (
      payload.version !== 1 ||
      typeof payload.attemptId !== "string" ||
      !ATTEMPT_ID_PATTERN.test(payload.attemptId) ||
      typeof payload.expectedGeneration !== "number" ||
      !Number.isSafeInteger(payload.expectedGeneration) ||
      payload.expectedGeneration < 1 ||
      typeof payload.expiresAt !== "number" ||
      !Number.isSafeInteger(payload.expiresAt) ||
      payload.expiresAt <= now ||
      typeof payload.touchGrassId !== "string" ||
      !PUBLIC_ID_PATTERN.test(payload.touchGrassId)
    ) {
      return null;
    }
    return payload as RecoveryProofPayload;
  } catch {
    return null;
  }
}

function stringField(value: unknown, field: string) {
  if (!value || typeof value !== "object") return null;
  const candidate = Reflect.get(value, field);
  return typeof candidate === "string" ? candidate : null;
}

function recoveryUser(value: unknown) {
  if (!value || typeof value !== "object") return null;
  const id = Reflect.get(value, "id");
  return typeof id === "string" ? { id } : null;
}

function recoveryCredentialAccount(value: unknown) {
  if (!value || typeof value !== "object") return null;
  const id = Reflect.get(value, "id");
  const password = Reflect.get(value, "password");
  return typeof id === "string" && typeof password === "string"
    ? { id, password }
    : null;
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

function rejectRecoveryCredential(): never {
  throw new APIError("UNAUTHORIZED", {
    message: "Profile recovery failed. Try again.",
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
      prepareTouchGrassRecovery: createAuthEndpoint(
        "/touchgrass/recovery/prepare",
        { method: "POST" },
        async (ctx) => {
          const touchGrassId = stringField(ctx.body, "touchGrassId");
          const recoveryKey = stringField(ctx.body, "recoveryKey");
          const attemptId = stringField(ctx.body, "attemptId");
          const ipAddress = await requestIpAddress(policy);
          const keys = await failedCredentialKeys(
            ctx.context.secret,
            ipAddress,
            touchGrassId,
          );
          const reservationId = await policy
            .reserveCredentialAttempt(keys)
            .catch(() => null);
          if (!reservationId) rejectRateLimitedCredential();

          const validShape =
            touchGrassId !== null &&
            PUBLIC_ID_PATTERN.test(touchGrassId) &&
            recoveryKey !== null &&
            RECOVERY_KEY_PATTERN.test(recoveryKey) &&
            attemptId !== null &&
            ATTEMPT_ID_PATTERN.test(attemptId);
          const user = recoveryUser(
            validShape
              ? await ctx.context.adapter.findOne({
                  model: "user",
                  where: [{ field: "username", value: touchGrassId }],
                })
              : null,
          );
          const account = recoveryCredentialAccount(
            user
              ? await ctx.context.adapter.findOne({
                  model: "account",
                  where: [
                    { field: "userId", value: user.id },
                    { field: "providerId", value: "credential" },
                  ],
                })
              : null,
          );
          const credentialIsValid =
            recoveryKey !== null &&
            typeof account?.password === "string" &&
            (await ctx.context.password.verify({
              hash: account.password,
              password: recoveryKey,
            }));
          if (!credentialIsValid || !user || !touchGrassId || !attemptId) {
            if (!account?.password) {
              await ctx.context.password.hash(
                recoveryKey ?? "invalid-recovery-credential",
              );
            }
            await policy.finalizeCredentialAttempt({
              outcome: "failure",
              reservationId,
            });
            return rejectRecoveryCredential();
          }

          const attemptDigest = await sha256Digest(attemptId);
          const prepared = await policy.prepareRecoveryAttempt({
            attemptDigest,
            authSubject: user.id,
            touchGrassId,
          });
          if (!prepared) {
            await policy.finalizeCredentialAttempt({
              outcome: "failure",
              reservationId,
            });
            return rejectRecoveryCredential();
          }
          const finalized = await policy.finalizeCredentialAttempt({
            outcome: "success",
            reservationId,
          });
          if (!finalized) rejectRateLimitedCredential();
          const recoveryProof = await signRecoveryProof(ctx.context.secret, {
            attemptId,
            expectedGeneration: prepared.expectedGeneration,
            expiresAt: prepared.expiresAt,
            touchGrassId,
            version: 1,
          });
          return ctx.json({ recoveryProof });
        },
      ),
      commitTouchGrassRecovery: createAuthEndpoint(
        "/touchgrass/recovery/commit",
        { method: "POST" },
        async (ctx) => {
          const currentRecoveryKey = stringField(
            ctx.body,
            "currentRecoveryKey",
          );
          const installationCredential = stringField(
            ctx.body,
            "installationCredential",
          );
          const newRecoveryKey = stringField(ctx.body, "newRecoveryKey");
          const recoveryProof = stringField(ctx.body, "recoveryProof");
          const proof = recoveryProof
            ? await verifyRecoveryProof(
                ctx.context.secret,
                recoveryProof,
                Date.now(),
              )
            : null;
          const ipAddress = await requestIpAddress(policy);
          const keys = await failedCredentialKeys(
            ctx.context.secret,
            ipAddress,
            proof?.touchGrassId,
          );
          const reservationId = await policy
            .reserveCredentialAttempt(keys)
            .catch(() => null);
          if (!reservationId) rejectRateLimitedCredential();

          const validShape =
            proof !== null &&
            currentRecoveryKey !== null &&
            RECOVERY_KEY_PATTERN.test(currentRecoveryKey) &&
            newRecoveryKey !== null &&
            RECOVERY_KEY_PATTERN.test(newRecoveryKey) &&
            installationCredential !== null &&
            INSTALLATION_CREDENTIAL_PATTERN.test(installationCredential);
          const user = recoveryUser(
            validShape
              ? await ctx.context.adapter.findOne({
                  model: "user",
                  where: [{ field: "username", value: proof.touchGrassId }],
                })
              : null,
          );
          const account = recoveryCredentialAccount(
            user
              ? await ctx.context.adapter.findOne({
                  model: "account",
                  where: [
                    { field: "userId", value: user.id },
                    { field: "providerId", value: "credential" },
                  ],
                })
              : null,
          );
          const currentKeyIsValid =
            currentRecoveryKey !== null &&
            typeof account?.password === "string" &&
            (await ctx.context.password.verify({
              hash: account.password,
              password: currentRecoveryKey,
            }));
          const replacementKeyIsCurrent =
            newRecoveryKey !== null &&
            typeof account?.password === "string" &&
            (await ctx.context.password.verify({
              hash: account.password,
              password: newRecoveryKey,
            }));
          if (
            !validShape ||
            !proof ||
            !user ||
            !account ||
            (!currentKeyIsValid && !replacementKeyIsCurrent)
          ) {
            if (!account?.password) {
              await ctx.context.password.hash(
                currentRecoveryKey ?? "invalid-recovery-credential",
              );
            }
            await policy.finalizeCredentialAttempt({
              outcome: "failure",
              reservationId,
            });
            return rejectRecoveryCredential();
          }

          const attemptDigest = await sha256Digest(proof.attemptId);
          const claimed = await policy.claimRecoveryAttempt({
            attemptDigest,
            authSubject: user.id,
          });
          if (!claimed) {
            await policy.finalizeCredentialAttempt({
              outcome: "failure",
              reservationId,
            });
            return rejectRecoveryCredential();
          }
          if (!replacementKeyIsCurrent) {
            const password = await ctx.context.password.hash(newRecoveryKey);
            await ctx.context.internalAdapter.updateAccount(account.id, {
              password,
            });
          }
          await ctx.context.internalAdapter.deleteUserSessions(user.id);
          const session = await ctx.context.internalAdapter.createSession(user.id);
          if (!session) {
            throw new APIError("INTERNAL_SERVER_ERROR", {
              message: "Profile recovery is unavailable.",
            });
          }
          const committed = await policy.commitRecoveryAttempt({
            attemptDigest,
            authSubject: user.id,
            installationCredential,
          });
          if (!committed) {
            await policy.finalizeCredentialAttempt({
              outcome: "failure",
              reservationId,
            });
            return rejectRecoveryCredential();
          }
          const finalized = await policy.finalizeCredentialAttempt({
            outcome: "success",
            reservationId,
          });
          if (!finalized) rejectRateLimitedCredential();
          return ctx.json({ ...committed, token: session.token });
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
