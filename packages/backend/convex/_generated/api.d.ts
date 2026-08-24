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
import type * as auth_profileRecovery from "../auth/profileRecovery.js";
import type * as auth_touchgrassSignup from "../auth/touchgrassSignup.js";
import type * as crons from "../crons.js";
import type * as doomerboards from "../doomerboards.js";
import type * as http from "../http.js";
import type * as internal_doomerboardInvariant from "../internal/doomerboardInvariant.js";
import type * as internal_doomerboardInvariantPage from "../internal/doomerboardInvariantPage.js";
import type * as internal_migrations from "../internal/migrations.js";
import type * as internal_profileAuthSessionFenceInvariant from "../internal/profileAuthSessionFenceInvariant.js";
import type * as internal_profileAuthSessionFencePagination from "../internal/profileAuthSessionFencePagination.js";
import type * as internal_recompute from "../internal/recompute.js";
import type * as model_authority from "../model/authority.js";
import type * as model_doomerboard from "../model/doomerboard.js";
import type * as model_doomerboardVersion from "../model/doomerboardVersion.js";
import type * as model_profile from "../model/profile.js";
import type * as model_rateLimits from "../model/rateLimits.js";
import type * as model_scores from "../model/scores.js";
import type * as model_sync from "../model/sync.js";
import type * as model_touchGrassId from "../model/touchGrassId.js";
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
  "auth/profileRecovery": typeof auth_profileRecovery;
  "auth/touchgrassSignup": typeof auth_touchgrassSignup;
  crons: typeof crons;
  doomerboards: typeof doomerboards;
  http: typeof http;
  "internal/doomerboardInvariant": typeof internal_doomerboardInvariant;
  "internal/doomerboardInvariantPage": typeof internal_doomerboardInvariantPage;
  "internal/migrations": typeof internal_migrations;
  "internal/profileAuthSessionFenceInvariant": typeof internal_profileAuthSessionFenceInvariant;
  "internal/profileAuthSessionFencePagination": typeof internal_profileAuthSessionFencePagination;
  "internal/recompute": typeof internal_recompute;
  "model/authority": typeof model_authority;
  "model/doomerboard": typeof model_doomerboard;
  "model/doomerboardVersion": typeof model_doomerboardVersion;
  "model/profile": typeof model_profile;
  "model/rateLimits": typeof model_rateLimits;
  "model/scores": typeof model_scores;
  "model/sync": typeof model_sync;
  "model/touchGrassId": typeof model_touchGrassId;
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
