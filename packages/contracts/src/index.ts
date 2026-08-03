import * as z from "zod";

export const codingProviderSchema = z.enum(["codex", "claude"]);
export type CodingProvider = z.infer<typeof codingProviderSchema>;

export const freshnessSchema = z.enum([
  "authoritative",
  "observed",
  "stale",
  "unavailable",
]);

export const quotaLaneSchema = z.object({
  label: z.string().min(1),
  unit: z.string().min(1),
  allowance: z.number().nonnegative().nullable(),
  remaining: z.number().nonnegative().nullable(),
  resetAt: z.string().datetime().nullable(),
});

export const providerSnapshotSchema = z.object({
  provider: codingProviderSchema,
  detected: z.boolean(),
  freshness: freshnessSchema,
  observedAt: z.string().datetime(),
  quotaLanes: z.array(quotaLaneSchema),
});

export const usageTotalSchema = z.object({
  observedTokens: z.number().int().nonnegative(),
  apiEquivalentCostUsd: z.number().nonnegative().nullable(),
  costIsComplete: z.boolean(),
});

export const usagePeriodSchema = z.object({
  today: usageTotalSchema,
  sevenDays: usageTotalSchema,
  thirtyDays: usageTotalSchema,
});

export const tokenmaxxerSchema = z.object({
  touchGrassId: z.string().regex(/^TG-[A-Z0-9]{6}$/),
  displayName: z.string().trim().min(1).max(40),
});

export const doomerboardRowSchema = tokenmaxxerSchema.extend({
  rank: z.number().int().positive(),
  tokenScore: z.number().int().nonnegative(),
  apiEquivalentCostUsd: z.number().nonnegative().nullable(),
});

export const sanitizedDesktopStateSchema = z.object({
  contractVersion: z.literal(1),
  generatedAt: z.string().datetime(),
  providers: z.array(providerSnapshotSchema),
  usage: z.record(codingProviderSchema, usagePeriodSchema),
  sync: z.object({
    status: z.enum(["synced", "pending", "stale", "unavailable"]),
    lastSuccessfulAt: z.string().datetime().nullable(),
  }),
}).strict();

export type QuotaLane = z.infer<typeof quotaLaneSchema>;
export type ProviderSnapshot = z.infer<typeof providerSnapshotSchema>;
export type UsageTotal = z.infer<typeof usageTotalSchema>;
export type UsagePeriod = z.infer<typeof usagePeriodSchema>;
export type Tokenmaxxer = z.infer<typeof tokenmaxxerSchema>;
export type DoomerboardRow = z.infer<typeof doomerboardRowSchema>;
export type SanitizedDesktopState = z.infer<typeof sanitizedDesktopStateSchema>;
