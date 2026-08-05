import { createClient, type GenericCtx } from "@convex-dev/better-auth";
import { convex } from "@convex-dev/better-auth/plugins";
import { betterAuth } from "better-auth/minimal";
import { bearer } from "better-auth/plugins/bearer";
import { username } from "better-auth/plugins/username";

import { components } from "./_generated/api";
import type { DataModel } from "./_generated/dataModel";
import { env } from "./_generated/server";
import authConfig from "./auth.config";
import { touchGrassSignup } from "./auth/touchgrassSignup";

export const authComponent = createClient<DataModel>(components.betterAuth);

export const createAuth = (ctx: GenericCtx<DataModel>) =>
  betterAuth({
    baseURL: env.CONVEX_SITE_URL,
    database: authComponent.adapter(ctx),
    disabledPaths: ["/is-username-available"],
    emailAndPassword: {
      autoSignIn: false,
      enabled: true,
      requireEmailVerification: false,
    },
    plugins: [
      touchGrassSignup(),
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
    secret: env.BETTER_AUTH_SECRET,
  });

export async function requireAuthUser(ctx: GenericCtx<DataModel>) {
  const user = await authComponent.getAuthUser(ctx);
  return { ...user, id: user._id };
}
