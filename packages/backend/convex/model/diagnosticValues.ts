import {
  DIAGNOSTIC_DATABASE_MODULES,
  DIAGNOSTIC_DATABASE_STAGES,
  DIAGNOSTIC_PARSER_REASONS,
  DIAGNOSTIC_PRICING_REASONS,
  DIAGNOSTIC_SYNC_REASONS,
  type DiagnosticReport,
} from "@touchgrass/contracts";
import { type Infer, v } from "convex/values";

import { providerValidator } from "./values";

function literals<const T extends readonly string[]>(values: T) {
  return v.union(...values.map((value) => v.literal(value as T[number])));
}

const nullableNumber = v.union(v.number(), v.null());
const nullableString = v.union(v.string(), v.null());

export const diagnosticFailureValidator = v.union(
  v.object({
    area: v.literal("database"),
    provider: v.null(),
    code: literals([
      "database_open_failed",
      "database_operation_failed",
      "database_migration_failed",
      "database_invariant_failed",
      "database_version_unsupported",
    ]),
    context: v.object({
      stage: v.union(literals(DIAGNOSTIC_DATABASE_STAGES), v.null()),
      observedFormat: nullableNumber,
      expectedFormat: nullableNumber,
      modules: v.array(
        v.object({
          module: literals(DIAGNOSTIC_DATABASE_MODULES),
          observedVersion: nullableNumber,
          expectedVersion: nullableNumber,
        }),
      ),
      backupState: literals(["absent", "present", "validated", "invalid", "unknown"]),
    }),
  }),
  v.object({
    area: v.literal("parser"),
    provider: providerValidator,
    code: literals(["parser_scan_failed", "parser_record_invalid", "parser_invariant_failed"]),
    context: v.object({
      parserVersion: nullableNumber,
      sourceVersions: v.array(v.string()),
      reviewStatus: literals(["reviewed", "unreviewed", "mixed", "unknown"]),
      filesSeen: nullableNumber,
      recordsAccepted: nullableNumber,
      recordsRejected: nullableNumber,
      reason: literals(DIAGNOSTIC_PARSER_REASONS),
    }),
  }),
  v.object({
    area: v.literal("pricing"),
    provider: providerValidator,
    code: literals(["pricing_calculation_failed", "pricing_catalog_not_approved"]),
    context: v.object({
      rankingDay: nullableString,
      revision: nullableNumber,
      parserVersion: nullableNumber,
      catalogVersion: nullableString,
      reason: literals(DIAGNOSTIC_PRICING_REASONS),
      observedTokens: nullableNumber,
      pricedTokens: nullableNumber,
      localCostMicros: nullableNumber,
      outgoingCostMicros: nullableNumber,
    }),
  }),
  v.object({
    area: v.literal("sync"),
    provider: v.union(providerValidator, v.null()),
    code: literals(["sync_request_failed", "sync_revision_conflict"]),
    context: v.object({
      stage: literals(["provider_settings", "daily_usage"]),
      reason: literals(DIAGNOSTIC_SYNC_REASONS),
      statusCode: nullableNumber,
      rankingDay: nullableString,
      attemptedRevision: nullableNumber,
      lastAcknowledgedRevision: nullableNumber,
      pendingCount: nullableNumber,
    }),
  }),
  v.object({
    area: v.literal("provider_access"),
    provider: providerValidator,
    code: v.literal("provider_access_failed"),
    context: v.object({
      operation: literals(["read_usage", "read_quota", "refresh_credentials"]),
      reason: literals([
        "read_failed",
        "permission_denied",
        "request_failed",
        "invalid_response",
        "credentials_rejected",
      ]),
      statusCode: nullableNumber,
      retryCount: v.number(),
    }),
  }),
);

export const diagnosticReportValidator = v.object({
  schemaVersion: v.literal(1),
  reportId: v.string(),
  firstOccurredAt: v.number(),
  lastOccurredAt: v.number(),
  contextCapturedAt: v.number(),
  occurrenceCount: v.number(),
  app: v.object({
    version: v.string(),
    build: nullableString,
    osVersion: nullableString,
    architecture: literals(["aarch64", "x86_64", "unknown"]),
  }),
  failure: diagnosticFailureValidator,
});

// A changed wire shape must also change the registered Convex validator.
type ValidatorReport = Infer<typeof diagnosticReportValidator>;
type Assert<T extends true> = T;
export type DiagnosticValidatorMatchesContract = Assert<
  ValidatorReport extends DiagnosticReport
    ? DiagnosticReport extends ValidatorReport
      ? true
      : false
    : false
>;

export const diagnosticStoredReportValidator = v.object({
  _id: v.id("diagnosticReports"),
  _creationTime: v.number(),
  reporterId: v.id("diagnosticReporters"),
  tokenmaxxerId: v.id("tokenmaxxers"),
  deviceId: v.id("devices"),
  generation: v.number(),
  reportId: v.string(),
  payloadDigest: v.string(),
  groupKey: v.string(),
  provider: v.union(providerValidator, v.null()),
  receivedAt: v.number(),
  expiresAt: v.number(),
  report: diagnosticReportValidator,
});
