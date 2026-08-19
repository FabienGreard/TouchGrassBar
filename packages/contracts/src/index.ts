export * from "./native.generated";

import * as z from "zod";

export const tokenmaxxerSchema = z
  .object({
    touchGrassId: z.string().regex(/^TG-[23456789ABCDEFGHJKLMNPQRSTUVWXYZ]{6}$/),
    displayName: z.string().trim().min(1).max(40),
  })
  .strict();

export const doomerboardRowSchema = tokenmaxxerSchema
  .extend({
    rank: z.number().int().positive(),
    tokenScore: z.number().int().nonnegative(),
    apiEquivalentCostUsd: z.number().nonnegative().nullable(),
  })
  .strict();

export type Tokenmaxxer = z.infer<typeof tokenmaxxerSchema>;
export type DoomerboardRow = z.infer<typeof doomerboardRowSchema>;
