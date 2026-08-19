import { createClient, type GenericCtx } from "@convex-dev/better-auth";
import { convex } from "@convex-dev/better-auth/plugins";
import { ConvexError } from "convex/values";
import { betterAuth } from "better-auth/minimal";
import { bearer } from "better-auth/plugins/bearer";
import { username } from "better-auth/plugins/username";

import { components, internal } from "./_generated/api";
import type { DataModel } from "./_generated/dataModel";
import { env, type ActionCtx } from "./_generated/server";
import authConfig from "./auth.config";
import { touchGrassSignup, type TouchGrassPolicyPort } from "./auth/touchgrassSignup";
import { rejectAuthority } from "./model/authority";
import { rateLimiter } from "./model/rateLimits";

declare const process: {
  env: { readonly CONVEX_SITE_URL?: string };
};

export const authComponent = createClient<DataModel>(components.betterAuth);

function actionContext(ctx: GenericCtx<DataModel>): ActionCtx {
  if (!("runAction" in ctx)) {
    throw new Error("TouchGrass authentication requires an action context");
  }
  return ctx;
}

function touchGrassPolicy(
  ctx: GenericCtx<DataModel>,
  requestIpAddress: () => Promise<string | null>,
): TouchGrassPolicyPort {
  const action = () => actionContext(ctx);

  return {
    claimRecoveryAuthFinalization: (args) =>
      action().runMutation(
        internal.auth.profileRecovery.claimRecoveryAuthFinalization,
        args,
      ),
    claimRecoveryAttempt: (args) =>
      action().runMutation(
        internal.auth.profileRecovery.claimRecoveryAttempt,
        args,
      ),
    commitRecoveryAttempt: (args) =>
      action().runMutation(
        internal.auth.profileRecovery.commitRecoveryAttempt,
        args,
      ),
    finalizeRecoveryAuth: (args) =>
      action().runMutation(
        internal.auth.profileRecovery.finalizeRecoveryAuth,
        args,
      ),
    releaseRecoveryAuthFinalization: async (args) => {
      await action().runMutation(
        internal.auth.profileRecovery.releaseRecoveryAuthFinalization,
        args,
      );
    },
    consumeSignupProof: (args) =>
      action().runMutation(internal.auth.touchgrassSignup.consumeSignupProof, args),
    finalizeCredentialAttempt: (args) =>
      action().runMutation(internal.auth.credentialAttempts.finalizeCredentialAttempt, args),
    issueSignupProof: async (args) => {
      await action().runMutation(internal.auth.touchgrassSignup.issueSignupProof, args);
    },
    limitProfilePreparation: async ({ ipKey }) => {
      const limit = await rateLimiter.limit(action(), "profilePreparationByIp", { key: ipKey });
      return limit.ok;
    },
    requestIpAddress,
    prepareRecoveryAttempt: (args) =>
      action().runMutation(
        internal.auth.profileRecovery.prepareRecoveryAttempt,
        args,
      ),
    recoveryAuthPending: (args) =>
      action().runQuery(
        internal.auth.profileRecovery.recoveryAuthPending,
        args,
      ),
    reserveCredentialAttempt: (args) =>
      action().runMutation(internal.auth.credentialAttempts.reserveCredentialAttempt, args),
  };
}

export const createAuthWithRequestIp = (
  ctx: GenericCtx<DataModel>,
  requestIpAddress: () => Promise<string | null>,
) => {
  const convexSiteUrl = process.env.CONVEX_SITE_URL;
  if (!convexSiteUrl) throw new Error("CONVEX_SITE_URL is unavailable");
  return betterAuth({
    baseURL: convexSiteUrl,
    database: authComponent.adapter(ctx),
    disabledPaths: [
      "/change-email",
      "/change-password",
      "/delete-user",
      "/delete-user/callback",
      "/is-username-available",
      "/link-social",
      "/request-password-reset",
      "/reset-password",
      "/sign-in/email",
      "/unlink-account",
      "/update-user",
    ],
    emailAndPassword: {
      autoSignIn: false,
      enabled: true,
      requireEmailVerification: false,
    },
    plugins: [
      touchGrassSignup(touchGrassPolicy(ctx, requestIpAddress)),
      username({
        maxUsernameLength: 9,
        minUsernameLength: 9,
        usernameNormalization: (value) => value.toUpperCase(),
        usernameValidator: (value) => /^TG-[A-HJ-NP-Z2-9]{6}$/.test(value),
        validationOrder: { username: "post-normalization" },
      }),
      bearer(),
      convex({
        authConfig,
        jwt: {
          definePayload: () => ({}),
          expirationSeconds: 5 * 60,
        },
      }),
    ],
    rateLimit: { enabled: false },
    secret: env.BETTER_AUTH_SECRET,
  });
};

export const createAuth = (ctx: GenericCtx<DataModel>) =>
  createAuthWithRequestIp(ctx, async () => {
    const metadata = await actionContext(ctx).meta.getRequestMetadata();
    return metadata.ip;
  });

export async function requireAuthUser(ctx: GenericCtx<DataModel>) {
  const identity = await ctx.auth.getUserIdentity();
  if (!identity) return rejectAuthority();
  try {
    const user = await authComponent.getAuthUser(ctx);
    return { ...user, id: user._id };
  } catch (error) {
    if (error instanceof ConvexError && error.data === "Unauthenticated") {
      return rejectAuthority();
    }
    throw error;
  }
}
