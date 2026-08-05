/* eslint-disable */
/**
 * Generated `api` utility.
 *
 * THIS CODE IS AUTOMATICALLY GENERATED.
 *
 * To regenerate, run `npx convex dev`.
 * @module
 */

import type * as auth from "../auth.js";
import type * as auth_credentialAttempts from "../auth/credentialAttempts.js";
import type * as auth_touchgrassSignup from "../auth/touchgrassSignup.js";
import type * as crons from "../crons.js";
import type * as doomerboards from "../doomerboards.js";
import type * as http from "../http.js";
import type * as internal_migrations from "../internal/migrations.js";
import type * as internal_recompute from "../internal/recompute.js";
import type * as model_aggregate from "../model/aggregate.js";
import type * as model_profile from "../model/profile.js";
import type * as model_rateLimits from "../model/rateLimits.js";
import type * as model_scores from "../model/scores.js";
import type * as model_sync from "../model/sync.js";
import type * as model_values from "../model/values.js";
import type * as sync from "../sync.js";
import type * as tokenmaxxers from "../tokenmaxxers.js";

import type {
  ApiFromModules,
  FilterApi,
  FunctionReference,
} from "convex/server";

declare const fullApi: ApiFromModules<{
  auth: typeof auth;
  "auth/credentialAttempts": typeof auth_credentialAttempts;
  "auth/touchgrassSignup": typeof auth_touchgrassSignup;
  crons: typeof crons;
  doomerboards: typeof doomerboards;
  http: typeof http;
  "internal/migrations": typeof internal_migrations;
  "internal/recompute": typeof internal_recompute;
  "model/aggregate": typeof model_aggregate;
  "model/profile": typeof model_profile;
  "model/rateLimits": typeof model_rateLimits;
  "model/scores": typeof model_scores;
  "model/sync": typeof model_sync;
  "model/values": typeof model_values;
  sync: typeof sync;
  tokenmaxxers: typeof tokenmaxxers;
}>;

/**
 * A utility for referencing Convex functions in your app's public API.
 *
 * Usage:
 * ```js
 * const myFunctionReference = api.myModule.myFunction;
 * ```
 */
export declare const api: FilterApi<
  typeof fullApi,
  FunctionReference<any, "public">
>;

/**
 * A utility for referencing Convex functions in your app's internal API.
 *
 * Usage:
 * ```js
 * const myFunctionReference = internal.myModule.myFunction;
 * ```
 */
export declare const internal: FilterApi<
  typeof fullApi,
  FunctionReference<any, "internal">
>;

export declare const components: {
  doomerboard: import("@convex-dev/aggregate/_generated/component.js").ComponentApi<"doomerboard">;
  betterAuth: import("@convex-dev/better-auth/_generated/component.js").ComponentApi<"betterAuth">;
  migrations: import("@convex-dev/migrations/_generated/component.js").ComponentApi<"migrations">;
  rateLimiter: import("@convex-dev/rate-limiter/_generated/component.js").ComponentApi<"rateLimiter">;
};
