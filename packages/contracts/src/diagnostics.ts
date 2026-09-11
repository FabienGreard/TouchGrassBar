import * as z from "zod";

import { codingProviderSchema } from "./native.generated";

export const DIAGNOSTIC_MAX_BYTES = 32 * 1024;
export const DIAGNOSTIC_LOCAL_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000;
export const DIAGNOSTIC_SERVER_RETENTION_MS = 14 * 24 * 60 * 60 * 1000;
export const DIAGNOSTIC_MAX_FUTURE_SKEW_MS = 5 * 60 * 1000;

// These are source-code labels, never names discovered in a user's database.
export const DIAGNOSTIC_DATABASE_MODULES = [
  "database-coordinator",
  "desktop-lifecycle",
  "sanitized-desktop-state",
  "codex-usage-index",
  "claude-usage-index",
  "update-state",
  "unregistered-module",
] as const;

export const DIAGNOSTIC_DATABASE_STAGES = [
  ...DIAGNOSTIC_DATABASE_MODULES,
  "database-format",
  "unregistered-object",
  "open-lifecycle",
  "open-native-core",
  "open-database",
  "open-ready",
  "open-source",
  "open-backup",
  "open-backup-source",
  "inspect-source",
  "inspect-format",
  "inspect-version-vector",
  "inspect-desktop-lifecycle",
  "inspect-sanitized-state",
  "inspect-codex-usage",
  "inspect-claude-usage",
  "inspect-update-state",
  "inspect-table-columns",
  "inspect-objects",
  "inspect-object-definitions",
  "after-backup",
  "provider-usage-indexes",
  "open-final",
  "configure-final",
  "begin-final",
  "write-version-vector",
  "write-database-format",
  "before-final-commit",
  "commit-final",
  "after-final-commit",
  "replace-partial-backup",
  "copy-backup",
  "before-backup-complete",
  "sync-backup",
  "publish-backup",
  "sync-backup-directory",
  "replace-partial-marker",
  "write-migration-marker",
  "publish-migration-marker",
  "validate-backup",
  "finish-migration",
  "validate-backup-source",
  "prune-module-backups",
  "sync-marker-directory",
  "sync-migration-directory",
  "integrity",
  "foreign-keys",
  "version-vector",
  "object-registry",
  "table-registry",
  "index-registry",
  "view-registry",
  "sanitized-state",
  "profile-projection",
  "table-columns",
  "table-definitions",
  "foreign-key-definitions",
  "index-definitions",
  "view-definitions",
  "lifecycle-state",
  "provider-settings",
  "usage-sync-values",
  "codex-usage-values",
  "claude-usage-values",
] as const;

export const DIAGNOSTIC_PARSER_REASONS = [
  "read_failed",
  "invalid_json",
  "invalid_usage_shape",
  "invalid_counter",
  "unsupported_record",
  "invariant_failed",
  "record_too_large",
  "missing_cache_counters",
  "cache_split_mismatch",
  "iteration_shape_mismatch",
  "thinking_counter_invalid",
  "unknown_usage_fields",
  "fallback_credit_unsupported",
] as const;

export const DIAGNOSTIC_PRICING_REASONS = [
  "unknown_model",
  "catalog_unavailable",
  "catalog_not_approved",
  "missing_usage_metadata",
  "unsupported_modifier",
  "counter_overflow",
  "invalid_counter",
  "invalid_cost",
  "missing_cache_write_split",
  "missing_web_search_usage",
  "unpriced_code_execution",
  "unknown_paid_server_tool",
  "missing_effective_price",
  "unknown_service_tier",
  "unknown_inference_geo",
  "fast_batch_combination",
  "missing_fast_price",
  "missing_speed",
  "unknown_speed",
  "missing_cache_write_price",
] as const;

export const DIAGNOSTIC_SYNC_REASONS = [
  "transport_failed",
  "server_rejected",
  "authority_rejected",
  "revision_conflict",
  "invalid_response",
] as const;

const count = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const version = z.number().int().min(0).max(2_147_483_647);
const numericVersion = z
  .string()
  .min(1)
  .max(32)
  .regex(/^\d+(?:\.\d+){0,3}$/);
const sourceVersion = z
  .string()
  .min(1)
  .max(32)
  .regex(/^\d+(?:\.\d+){0,3}(?:-(?:alpha|beta|rc)\.\d+(?:\.\d+){0,2})?$/);
const rankingDay = z.iso.date();
const revision = count.min(1);
const statusCode = z.number().int().min(100).max(599).nullable();
const catalogVersion = z
  .string()
  .max(96)
  .regex(
    /^(?:(?:openai|anthropic)-(?:api|standard)-\d{4}-\d{2}-\d{2}-v[1-9]\d*|fnv1a64:[a-f0-9]{16}|sha256:[a-f0-9]{64})$/,
  );

export const diagnosticDatabaseContextSchema = z
  .object({
    stage: z.enum(DIAGNOSTIC_DATABASE_STAGES).nullable(),
    observedFormat: version.nullable(),
    expectedFormat: version.nullable(),
    modules: z
      .array(
        z
          .object({
            module: z.enum(DIAGNOSTIC_DATABASE_MODULES),
            observedVersion: version.nullable(),
            expectedVersion: version.nullable(),
          })
          .strict(),
      )
      .max(16),
    backupState: z.enum(["absent", "present", "validated", "invalid", "unknown"]),
  })
  .strict();

export const diagnosticParserContextSchema = z
  .object({
    parserVersion: version.nullable(),
    sourceVersions: z.array(sourceVersion).max(8),
    reviewStatus: z.enum(["reviewed", "unreviewed", "mixed", "unknown"]),
    filesSeen: count.nullable(),
    recordsAccepted: count.nullable(),
    recordsRejected: count.nullable(),
    reason: z.enum(DIAGNOSTIC_PARSER_REASONS),
  })
  .strict();

export const diagnosticPricingContextSchema = z
  .object({
    rankingDay: rankingDay.nullable(),
    revision: revision.nullable(),
    parserVersion: version.nullable(),
    catalogVersion: catalogVersion.nullable(),
    reason: z.enum(DIAGNOSTIC_PRICING_REASONS),
    observedTokens: count.nullable(),
    pricedTokens: count.nullable(),
    localCostMicros: count.nullable(),
    outgoingCostMicros: count.nullable(),
  })
  .strict();

export const diagnosticSyncContextSchema = z
  .object({
    stage: z.enum(["provider_settings", "daily_usage"]),
    reason: z.enum(DIAGNOSTIC_SYNC_REASONS),
    statusCode,
    rankingDay: rankingDay.nullable(),
    attemptedRevision: revision.nullable(),
    lastAcknowledgedRevision: revision.nullable(),
    pendingCount: count.nullable(),
  })
  .strict();

export const diagnosticProviderAccessContextSchema = z
  .object({
    operation: z.enum(["read_usage", "read_quota", "refresh_credentials"]),
    reason: z.enum([
      "read_failed",
      "permission_denied",
      "request_failed",
      "invalid_response",
      "credentials_rejected",
    ]),
    statusCode,
    retryCount: count,
  })
  .strict();

export const diagnosticFailureSchema = z.discriminatedUnion("area", [
  z
    .object({
      area: z.literal("database"),
      provider: z.null(),
      code: z.enum([
        "database_open_failed",
        "database_operation_failed",
        "database_migration_failed",
        "database_invariant_failed",
        "database_version_unsupported",
      ]),
      context: diagnosticDatabaseContextSchema,
    })
    .strict(),
  z
    .object({
      area: z.literal("parser"),
      provider: codingProviderSchema,
      code: z.enum(["parser_scan_failed", "parser_record_invalid", "parser_invariant_failed"]),
      context: diagnosticParserContextSchema,
    })
    .strict(),
  z
    .object({
      area: z.literal("pricing"),
      provider: codingProviderSchema,
      code: z.enum(["pricing_calculation_failed", "pricing_catalog_not_approved"]),
      context: diagnosticPricingContextSchema,
    })
    .strict(),
  z
    .object({
      area: z.literal("sync"),
      provider: codingProviderSchema.nullable(),
      code: z.enum(["sync_request_failed", "sync_revision_conflict"]),
      context: diagnosticSyncContextSchema,
    })
    .strict(),
  z
    .object({
      area: z.literal("provider_access"),
      provider: codingProviderSchema,
      code: z.literal("provider_access_failed"),
      context: diagnosticProviderAccessContextSchema,
    })
    .strict(),
]);

export const diagnosticReportSchema = z
  .object({
    schemaVersion: z.literal(1),
    reportId: z
      .string()
      .uuid()
      .regex(/^[a-f0-9-]+$/),
    firstOccurredAt: count,
    lastOccurredAt: count,
    contextCapturedAt: count,
    occurrenceCount: z.number().int().min(1).max(10_000),
    app: z
      .object({
        version: numericVersion,
        build: z
          .string()
          .min(1)
          .max(64)
          .regex(/^[a-f0-9]+$/)
          .nullable(),
        osVersion: numericVersion.nullable(),
        architecture: z.enum(["aarch64", "x86_64", "unknown"]),
      })
      .strict(),
    failure: diagnosticFailureSchema,
  })
  .strict()
  .superRefine((report, ctx) => {
    if (
      report.lastOccurredAt < report.firstOccurredAt ||
      report.contextCapturedAt < report.firstOccurredAt ||
      report.contextCapturedAt > report.lastOccurredAt
    ) {
      ctx.addIssue({ code: "custom", message: "Invalid failure time order" });
    }
    const failure = report.failure;
    if (failure.area === "pricing") {
      const evidence = failure.context;
      if (
        evidence.observedTokens !== null &&
        evidence.pricedTokens !== null &&
        evidence.pricedTokens > evidence.observedTokens
      ) {
        ctx.addIssue({ code: "custom", message: "Priced tokens exceed observed tokens" });
      }
      if (
        failure.code === "pricing_catalog_not_approved" &&
        (evidence.reason !== "catalog_not_approved" ||
          evidence.localCostMicros === null ||
          evidence.outgoingCostMicros !== null)
      ) {
        ctx.addIssue({ code: "custom", message: "Invalid dropped cost evidence" });
      }
    }
    if (failure.area === "sync") {
      if (
        (failure.context.stage === "provider_settings" &&
          (failure.provider !== null || failure.context.rankingDay !== null)) ||
        (failure.context.stage === "daily_usage" &&
          failure.provider === null &&
          (failure.code !== "sync_request_failed" ||
            failure.context.rankingDay !== null ||
            failure.context.attemptedRevision !== null ||
            failure.context.lastAcknowledgedRevision !== null ||
            failure.context.pendingCount !== 0)) ||
        (failure.code === "sync_revision_conflict" &&
          failure.context.reason !== "revision_conflict")
      ) {
        ctx.addIssue({ code: "custom", message: "Invalid sync scope" });
      }
    }
  });

export type DiagnosticReport = z.infer<typeof diagnosticReportSchema>;
export type DiagnosticFailure = z.infer<typeof diagnosticFailureSchema>;
