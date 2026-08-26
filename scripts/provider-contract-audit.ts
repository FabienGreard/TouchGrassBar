#!/usr/bin/env bun

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export type ProviderAuditStatus = "pass" | "review-required" | "unavailable";
type Provider = "claude" | "codex";
type AuditArea = "parser" | "pricing" | "review";

export type ProviderAuditReasonCode =
  | "effective-date-changed"
  | "event-kind-changed"
  | "findings-truncated"
  | "future-price-changed"
  | "invalid-local-contract"
  | "invalid-source"
  | "price-changed"
  | "pricing-evidence-changed"
  | "pricing-modifier-changed"
  | "review-overdue"
  | "reviewed-snapshot-changed"
  | "schema-changed"
  | "source-unavailable"
  | "token-semantics-changed"
  | "unknown-field"
  | "unknown-model"
  | "unknown-price"
  | "unsupported-version"
  | "upstream-release-changed";

export type ProviderAuditFinding = {
  area: AuditArea;
  code: ProviderAuditReasonCode;
  provider: Provider;
  sourceUrl?: string;
  status: Exclude<ProviderAuditStatus, "pass">;
  summary: string;
};

export type ProviderAuditReport = {
  checkedAt: string;
  findings: ProviderAuditFinding[];
  reviewedAt: string;
  schemaVersion: 1;
  sourceCount: number;
  status: ProviderAuditStatus;
};

type ReviewedSchema = {
  id: string;
  path: string;
  semanticSha256: string;
};

type ReviewedCodexRustAnchor = ReviewedSchema & {
  kind: "event-msg-variants" | "response-token-mapping";
};

type ReviewedSemanticSource = {
  id: string;
  markers: string[];
  url: string;
};

type PricingPeriodKind = "fast" | "fast-long-context" | "standard";

type ReviewedPricingCheckpoint = {
  boundary: "end" | "start";
  model: string;
  periodKind: PricingPeriodKind;
};

type ReviewedAbsentPricingCheckpoint = {
  boundary: "absent";
  date: string;
  marker: string;
  model: string;
  periodKind: PricingPeriodKind;
};

type ReviewedPricingEvidenceSection = {
  checkpoints: Array<ReviewedAbsentPricingCheckpoint | ReviewedPricingCheckpoint>;
  date: string;
  heading: string;
  id: string;
  selector: string;
  semanticSha256: string;
};

type ReviewedPricingEvidenceSource = {
  id: string;
  sections: ReviewedPricingEvidenceSection[];
  url: string;
  windowSemanticSha256: string;
};

type ReviewedPricingBoundaryExemption = ReviewedPricingCheckpoint & {
  date: string;
  reason: string;
};

type ReviewedPricingEvidence = {
  boundaryExemptions: ReviewedPricingBoundaryExemption[];
  sources: ReviewedPricingEvidenceSource[];
};

type ReviewedPricingEvidenceContract = {
  pricingEvidence: ReviewedPricingEvidence;
};

type ReviewedPricingRuleWindow = {
  endHeading: string;
  id: string;
  semanticSha256: string;
  startHeading: string;
};

type ReviewedProviderPricingContract = ReviewedPricingEvidenceContract & {
  pricingManifestPath: string;
  pricingManifestSemanticSha256: string;
  pricingRuleWindows: ReviewedPricingRuleWindow[];
  pricingSourceUrl: string;
};

export type ReviewedProviderContract = {
  schemaVersion: 1;
  reviewedAt: string;
  reviewEveryDays: number;
  codex: ReviewedProviderPricingContract & {
    modelCatalogPath: string;
    parserSourcePath: string;
    pricingSemanticMarkers: string[];
    releaseUrl: string;
    repositoryRawBaseUrl: string;
    reviewedRelease: string;
    rustAnchors: ReviewedCodexRustAnchor[];
    schemas: ReviewedSchema[];
  };
  claude: ReviewedProviderPricingContract & {
    agentSdkPackageUrl: string;
    latestPackageUrl: string;
    parserSourcePath: string;
    reviewedAgentSdkVersion: string;
    reviewedLatestVersion: string;
    reviewedStableVersion: string;
    stablePackageUrl: string;
    usageInterfaces: Record<string, Record<string, string>>;
    usageSemanticSources: ReviewedSemanticSource[];
    usageTypesSourceUrl: string;
  };
};

type FetchLike = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

export type ProviderAuditOptions = {
  contract?: ReviewedProviderContract;
  fetcher?: FetchLike;
  now?: Date;
  readText?: (path: string) => string;
  workspaceRoot?: string;
};

type AuditContext = {
  contract: ReviewedProviderContract;
  fetcher: FetchLike;
  findings: ProviderAuditFinding[];
  now: Date;
  readText: (path: string) => string;
  sources: Set<string>;
  workspaceRoot: string;
};

type SourceAuditDescriptor = {
  area: AuditArea;
  context: AuditContext;
  id: string;
  provider: Provider;
};

type OpenAiRate = {
  cacheWrite: number | null;
  cachedInput: number | null;
  input: number | null;
  longCacheWrite: number | null;
  longCachedInput: number | null;
  longInput: number | null;
  longOutput: number | null;
  output: number | null;
};

type PublishedOpenAiRate = OpenAiRate & {
  contextQualified: boolean;
};

type AnthropicRate = {
  cacheRead: number;
  cacheWrite1h: number;
  cacheWrite5m: number;
  input: number;
  output: number;
};

type OpenAiManifestPeriod = {
  cacheWriteUsdPerMillion: number | null;
  cachedInputUsdPerMillion: number;
  effectiveFrom: string;
  effectiveUntil: string | null;
  fastLongContext?: {
    cacheWriteUsdPerMillion: number | null;
    cachedInputUsdPerMillion: number;
    effectiveFrom: string;
    effectiveUntil: string | null;
    inputUsdPerMillion: number;
    outputUsdPerMillion: number;
  };
  fastMultiplier?: number;
  inputUsdPerMillion: number;
  longContext: {
    inputMultiplier: number;
    inputTokensAbove: number;
    outputMultiplier: number;
  };
  outputUsdPerMillion: number;
};

type OpenAiManifestModel = {
  aliases: string[];
  name: string;
  periods: OpenAiManifestPeriod[];
};

type OpenAiManifest = {
  basis: string;
  models: OpenAiManifestModel[];
  schemaVersion: number;
};

type AnthropicManifestPeriod = {
  cacheReadUsdPerMillion: number;
  cacheWrite1hUsdPerMillion: number;
  cacheWrite5mUsdPerMillion: number;
  effectiveFrom: string;
  effectiveUntil: string | null;
  inputUsdPerMillion: number;
  outputUsdPerMillion: number;
};

type AnthropicManifestModel = {
  aliases: string[];
  fastPeriods: AnthropicManifestPeriod[];
  name: string;
  standardPeriods: AnthropicManifestPeriod[];
  supportsUsInference: boolean;
};

type AnthropicManifest = {
  batchFactor: number;
  basis: string;
  models: AnthropicManifestModel[];
  schemaVersion: number;
  usInferenceFactor: number;
  webSearchUsdPerThousand: number;
};

const workspaceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const contractPath = resolve(
  workspaceRoot,
  "apps",
  "desktop",
  "src-tauri",
  "provider-contracts",
  "reviewed.json",
);
const allowedSourceHosts = new Set([
  "api.github.com",
  "developers.openai.com",
  "platform.claude.com",
  "platform.openai.com",
  "raw.githubusercontent.com",
  "registry.npmjs.org",
]);
const maxSourceBytes = 2 * 1024 * 1024;
const maxSourceUrlChars = 512;
const maxDetailedFindings = 48;
const maxFindingSummaryChars = 320;
const sourceTimeoutMs = 15_000;

class SourceShapeError extends Error {}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isIsoDayValue(value: unknown): value is string {
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}$/u.test(value)) return false;
  const parsed = new Date(`${value}T00:00:00.000Z`);
  return Number.isFinite(parsed.getTime()) && parsed.toISOString().slice(0, 10) === value;
}

function parseReviewedContract(source: string): ReviewedProviderContract {
  const value = JSON.parse(source) as unknown;
  if (
    !isRecord(value) ||
    value.schemaVersion !== 1 ||
    !isIsoDayValue(value.reviewedAt) ||
    typeof value.reviewEveryDays !== "number" ||
    !Number.isInteger(value.reviewEveryDays) ||
    value.reviewEveryDays < 1 ||
    value.reviewEveryDays > 365 ||
    !isRecord(value.codex) ||
    !isRecord(value.claude)
  ) {
    throw new Error("The reviewed provider contract is invalid.");
  }

  const codex = value.codex;
  const claude = value.claude;
  const isText = (record: Record<string, unknown>, key: string, max = 512) =>
    typeof record[key] === "string" &&
    (record[key] as string).length > 0 &&
    (record[key] as string).length <= max;
  const isRelativePath = (record: Record<string, unknown>, key: string) =>
    isText(record, key, 256) &&
    !(record[key] as string).startsWith("/") &&
    !(record[key] as string).split("/").includes("..");
  const isSha256 = (record: Record<string, unknown>, key: string) =>
    typeof record[key] === "string" && /^[a-f0-9]{64}$/u.test(record[key]);
  const hasValidMarkers = (record: Record<string, unknown>) =>
    Array.isArray(record.markers) &&
    record.markers.length > 0 &&
    record.markers.length <= 16 &&
    record.markers.every(
      (marker) =>
        typeof marker === "string" &&
        marker.length > 0 &&
        marker.length <= 512 &&
        plainMarkdown(marker).length > 0 &&
        !/[\u0000-\u001f\u007f]/u.test(marker),
    );
  const requiredCodexText = [
    "reviewedRelease",
    "releaseUrl",
    "repositoryRawBaseUrl",
    "modelCatalogPath",
    "parserSourcePath",
    "pricingManifestPath",
    "pricingSourceUrl",
  ];
  const requiredClaudeText = [
    "reviewedStableVersion",
    "reviewedLatestVersion",
    "stablePackageUrl",
    "latestPackageUrl",
    "reviewedAgentSdkVersion",
    "agentSdkPackageUrl",
    "usageTypesSourceUrl",
    "parserSourcePath",
    "pricingManifestPath",
    "pricingSourceUrl",
  ];
  if (
    requiredCodexText.some((key) => !isText(codex, key)) ||
    requiredClaudeText.some((key) => !isText(claude, key)) ||
    !isRelativePath(codex, "modelCatalogPath") ||
    !isRelativePath(codex, "parserSourcePath") ||
    !isRelativePath(codex, "pricingManifestPath") ||
    !isRelativePath(claude, "parserSourcePath") ||
    !isRelativePath(claude, "pricingManifestPath") ||
    !isSha256(codex, "pricingManifestSemanticSha256") ||
    !isSha256(claude, "pricingManifestSemanticSha256") ||
    !Array.isArray(codex.pricingRuleWindows) ||
    codex.pricingRuleWindows.length === 0 ||
    codex.pricingRuleWindows.length > 8 ||
    !Array.isArray(claude.pricingRuleWindows) ||
    claude.pricingRuleWindows.length === 0 ||
    claude.pricingRuleWindows.length > 8 ||
    !isRecord(codex.pricingEvidence) ||
    !Array.isArray(codex.pricingEvidence.sources) ||
    codex.pricingEvidence.sources.length === 0 ||
    codex.pricingEvidence.sources.length > 8 ||
    !Array.isArray(codex.pricingEvidence.boundaryExemptions) ||
    codex.pricingEvidence.boundaryExemptions.length > 256 ||
    !isRecord(claude.pricingEvidence) ||
    !Array.isArray(claude.pricingEvidence.sources) ||
    claude.pricingEvidence.sources.length === 0 ||
    claude.pricingEvidence.sources.length > 8 ||
    !Array.isArray(claude.pricingEvidence.boundaryExemptions) ||
    claude.pricingEvidence.boundaryExemptions.length > 256 ||
    !Array.isArray(codex.pricingSemanticMarkers) ||
    codex.pricingSemanticMarkers.length === 0 ||
    codex.pricingSemanticMarkers.length > 16 ||
    codex.pricingSemanticMarkers.some(
      (marker) =>
        typeof marker !== "string" ||
        marker.length === 0 ||
        marker.length > 512 ||
        plainMarkdown(marker).length === 0 ||
        /[\u0000-\u001f\u007f]/u.test(marker),
    ) ||
    !Array.isArray(codex.schemas) ||
    codex.schemas.length === 0 ||
    codex.schemas.length > 32 ||
    !Array.isArray(codex.rustAnchors) ||
    codex.rustAnchors.length === 0 ||
    codex.rustAnchors.length > 16 ||
    !isRecord(claude.usageInterfaces) ||
    Object.keys(claude.usageInterfaces).length === 0 ||
    Object.keys(claude.usageInterfaces).length > 16 ||
    !Array.isArray(claude.usageSemanticSources) ||
    claude.usageSemanticSources.length === 0 ||
    claude.usageSemanticSources.length > 8
  ) {
    throw new Error("The reviewed provider contract is invalid.");
  }

  for (const schema of codex.schemas) {
    if (
      !isRecord(schema) ||
      !safeIdentifier(schema.id, 128) ||
      !isRelativePath(schema, "path") ||
      !isSha256(schema, "semanticSha256")
    ) {
      throw new Error("The reviewed provider contract is invalid.");
    }
  }
  for (const anchor of codex.rustAnchors) {
    if (
      !isRecord(anchor) ||
      !safeIdentifier(anchor.id, 128) ||
      !isRelativePath(anchor, "path") ||
      !isSha256(anchor, "semanticSha256") ||
      !["event-msg-variants", "response-token-mapping"].includes(anchor.kind as string)
    ) {
      throw new Error("The reviewed provider contract is invalid.");
    }
  }
  for (const [name, fields] of Object.entries(claude.usageInterfaces)) {
    if (
      !safeIdentifier(name, 128) ||
      !isRecord(fields) ||
      Object.keys(fields).length === 0 ||
      Object.keys(fields).length > 512
    ) {
      throw new Error("The reviewed provider contract is invalid.");
    }
    for (const [field, signature] of Object.entries(fields)) {
      if (
        !safeIdentifier(field, 128) ||
        typeof signature !== "string" ||
        signature.length === 0 ||
        signature.length > 256 ||
        !/^(?:required|optional):/u.test(signature) ||
        /[\u0000-\u001f\u007f]/u.test(signature)
      ) {
        throw new Error("The reviewed provider contract is invalid.");
      }
    }
  }
  for (const semanticSource of claude.usageSemanticSources) {
    if (
      !isRecord(semanticSource) ||
      !safeIdentifier(semanticSource.id, 128) ||
      !isText(semanticSource, "url") ||
      !hasValidMarkers(semanticSource)
    ) {
      throw new Error("The reviewed provider contract is invalid.");
    }
  }
  for (const provider of [codex, claude]) {
    const windowIds = new Set<string>();
    for (const window of provider.pricingRuleWindows) {
      if (
        !isRecord(window) ||
        !safeIdentifier(window.id, 128) ||
        windowIds.has(window.id as string) ||
        !isText(window, "startHeading", 128) ||
        !/^#{1,4}\s+\S/u.test(window.startHeading as string) ||
        /[\u0000-\u001f\u007f]/u.test(window.startHeading as string) ||
        !isText(window, "endHeading", 128) ||
        !/^#{1,4}\s+\S/u.test(window.endHeading as string) ||
        /[\u0000-\u001f\u007f]/u.test(window.endHeading as string) ||
        window.startHeading === window.endHeading ||
        !isSha256(window, "semanticSha256")
      ) {
        throw new Error("The reviewed provider contract is invalid.");
      }
      windowIds.add(window.id as string);
    }
  }
  for (const [provider, evidence] of [
    ["codex", codex.pricingEvidence],
    ["claude", claude.pricingEvidence],
  ] as const) {
    const checkpointKeys = new Set<string>();
    const sourceIds = new Set<string>();
    for (const evidenceSource of evidence.sources) {
      if (
        !isRecord(evidenceSource) ||
        !safeIdentifier(evidenceSource.id, 128) ||
        sourceIds.has(evidenceSource.id as string) ||
        !isText(evidenceSource, "url") ||
        !isSha256(evidenceSource, "windowSemanticSha256") ||
        !Array.isArray(evidenceSource.sections) ||
        evidenceSource.sections.length === 0 ||
        evidenceSource.sections.length > 64
      ) {
        throw new Error("The reviewed provider contract is invalid.");
      }
      sourceIds.add(evidenceSource.id as string);
      const sectionIds = new Set<string>();
      for (const section of evidenceSource.sections) {
        if (
          !isRecord(section) ||
          !safeIdentifier(section.id, 128) ||
          sectionIds.has(section.id as string) ||
          !isText(section, "heading", 128) ||
          !/^###\s+\S/iu.test(section.heading as string) ||
          /[\u0000-\u001f\u007f]/u.test(section.heading as string) ||
          !isText(section, "selector") ||
          plainMarkdown(section.selector as string).length === 0 ||
          /[\u0000-\u001f\u007f]/u.test(section.selector as string) ||
          !isIsoDayValue(section.date) ||
          !isSha256(section, "semanticSha256") ||
          !Array.isArray(section.checkpoints) ||
          section.checkpoints.length === 0 ||
          section.checkpoints.length > 64
        ) {
          throw new Error("The reviewed provider contract is invalid.");
        }
        sectionIds.add(section.id as string);
        for (const checkpoint of section.checkpoints) {
          if (
            !isRecord(checkpoint) ||
            !safeIdentifier(checkpoint.model, 128) ||
            !["absent", "end", "start"].includes(checkpoint.boundary as string) ||
            !["fast", "fast-long-context", "standard"].includes(checkpoint.periodKind as string) ||
            (provider === "codex" && checkpoint.periodKind === "fast") ||
            (provider === "claude" && checkpoint.periodKind === "fast-long-context") ||
            (checkpoint.boundary === "absent" &&
              (!isIsoDayValue(checkpoint.date) ||
                typeof checkpoint.marker !== "string" ||
                checkpoint.marker.length === 0 ||
                checkpoint.marker.length > 512 ||
                plainMarkdown(checkpoint.marker).length === 0 ||
                /[\u0000-\u001f\u007f]/u.test(checkpoint.marker)))
          ) {
            throw new Error("The reviewed provider contract is invalid.");
          }
          const date = checkpoint.boundary === "absent" ? checkpoint.date : section.date;
          const key = `${checkpoint.model}|${checkpoint.periodKind}|${checkpoint.boundary}|${date}`;
          if (checkpointKeys.has(key)) {
            throw new Error("The reviewed provider contract is invalid.");
          }
          checkpointKeys.add(key);
        }
      }
    }
    for (const exemption of evidence.boundaryExemptions) {
      if (
        !isRecord(exemption) ||
        !safeIdentifier(exemption.model, 128) ||
        !["end", "start"].includes(exemption.boundary as string) ||
        !["fast", "fast-long-context", "standard"].includes(exemption.periodKind as string) ||
        (provider === "codex" && exemption.periodKind === "fast") ||
        (provider === "claude" && exemption.periodKind === "fast-long-context") ||
        !isIsoDayValue(exemption.date) ||
        typeof exemption.reason !== "string" ||
        exemption.reason.length === 0 ||
        exemption.reason.length > 256 ||
        plainMarkdown(exemption.reason).length === 0 ||
        /[\u0000-\u001f\u007f]/u.test(exemption.reason)
      ) {
        throw new Error("The reviewed provider contract is invalid.");
      }
      const key = `${exemption.model}|${exemption.periodKind}|${exemption.boundary}|${exemption.date}`;
      if (checkpointKeys.has(key)) {
        throw new Error("The reviewed provider contract is invalid.");
      }
      checkpointKeys.add(key);
    }
  }
  try {
    for (const url of [
      codex.releaseUrl,
      codex.repositoryRawBaseUrl,
      codex.pricingSourceUrl,
      ...codex.pricingEvidence.sources.map((source) => source.url),
      claude.stablePackageUrl,
      claude.latestPackageUrl,
      claude.agentSdkPackageUrl,
      claude.usageTypesSourceUrl,
      claude.pricingSourceUrl,
      ...claude.pricingEvidence.sources.map((source) => source.url),
      ...claude.usageSemanticSources.map((source) => source.url),
    ]) {
      sourceUrl(url as string);
    }
  } catch {
    throw new Error("The reviewed provider contract is invalid.");
  }
  return value as ReviewedProviderContract;
}

export function loadReviewedProviderContract(path = contractPath): ReviewedProviderContract {
  return parseReviewedContract(readFileSync(path, "utf8"));
}

function finding(
  context: AuditContext,
  provider: Provider,
  area: AuditArea,
  status: Exclude<ProviderAuditStatus, "pass">,
  code: ProviderAuditReasonCode,
  summary: string,
  sourceUrl?: string,
) {
  context.findings.push({
    area,
    code,
    provider,
    sourceUrl: sourceUrl && sourceUrl.length <= maxSourceUrlChars ? sourceUrl : undefined,
    status,
    summary:
      summary.length <= maxFindingSummaryChars
        ? summary
        : `${summary.slice(0, maxFindingSummaryChars - 3)}...`,
  });
}

function safeIdentifier(value: unknown, maxLength = 160): string | null {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > maxLength ||
    !/^[A-Za-z0-9._+:/%-]+$/u.test(value)
  ) {
    return null;
  }
  return value;
}

function sourceUrl(value: string): URL {
  if (value.length > maxSourceUrlChars) {
    throw new Error("The provider source URL is too long.");
  }
  const url = new URL(value);
  if (
    url.protocol !== "https:" ||
    !allowedSourceHosts.has(url.hostname) ||
    url.username !== "" ||
    url.password !== "" ||
    url.port !== ""
  ) {
    throw new Error("The provider source URL is not allowed.");
  }
  return url;
}

async function readBoundedResponseBody(response: Response): Promise<string> {
  const declaredLength = Number(response.headers.get("content-length"));
  if (Number.isFinite(declaredLength) && declaredLength > maxSourceBytes) {
    throw new Error("The provider source is too large.");
  }
  const reader = response.body?.getReader();
  if (!reader) throw new Error("The provider source is empty.");
  const decoder = new TextDecoder();
  const chunks: string[] = [];
  let bytes = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > maxSourceBytes) {
        await reader.cancel().catch(() => undefined);
        throw new Error("The provider source is too large.");
      }
      chunks.push(decoder.decode(value, { stream: true }));
    }
    chunks.push(decoder.decode());
  } finally {
    reader.releaseLock();
  }
  const body = chunks.join("");
  if (body.trim().length === 0) {
    throw new Error("The provider source is empty.");
  }
  return body;
}

async function fetchSource(
  descriptor: SourceAuditDescriptor,
  value: string,
): Promise<string | undefined> {
  const { area, context, id, provider } = descriptor;
  let url: URL;
  try {
    url = sourceUrl(value);
  } catch {
    finding(
      context,
      provider,
      area,
      "unavailable",
      "invalid-source",
      `${id}: the configured source URL is invalid.`,
    );
    return undefined;
  }
  context.sources.add(url.toString());
  try {
    const headers: Record<string, string> = {
      accept: "application/json, text/markdown, text/plain;q=0.9",
      "user-agent": "TouchGrassBar-provider-contract-audit/1",
    };
    const response = await context.fetcher(url, {
      headers,
      redirect: "error",
      signal: AbortSignal.timeout(sourceTimeoutMs),
    });
    const finalUrl = response.url ? sourceUrl(response.url) : url;
    if (!allowedSourceHosts.has(finalUrl.hostname) || !response.ok) {
      throw new Error("The provider source did not return a usable response.");
    }
    return await readBoundedResponseBody(response);
  } catch {
    finding(
      context,
      provider,
      area,
      "unavailable",
      "source-unavailable",
      `${id}: the authoritative public source is unavailable or unsafe to use.`,
      url.toString(),
    );
    return undefined;
  }
}

async function fetchJson(
  descriptor: SourceAuditDescriptor,
  url: string,
): Promise<unknown | undefined> {
  const { area, context, id, provider } = descriptor;
  const body = await fetchSource(descriptor, url);
  if (body === undefined) return undefined;
  try {
    return JSON.parse(body) as unknown;
  } catch {
    finding(
      context,
      provider,
      area,
      "unavailable",
      "invalid-source",
      `${id}: the first-party source is not valid JSON.`,
      url,
    );
    return undefined;
  }
}

function readLocalSource(
  descriptor: SourceAuditDescriptor,
  relativePath: string,
): string | undefined {
  const { area, context, id, provider } = descriptor;
  try {
    return context.readText(resolve(context.workspaceRoot, relativePath));
  } catch {
    finding(
      context,
      provider,
      area,
      "unavailable",
      "invalid-local-contract",
      `${id}: the reviewed local contract cannot be read.`,
    );
    return undefined;
  }
}

function parseJsonSource<T>(
  descriptor: SourceAuditDescriptor,
  source: string | undefined,
): T | undefined {
  const { area, context, id, provider } = descriptor;
  if (source === undefined) return undefined;
  try {
    const value = JSON.parse(source) as unknown;
    if (!isRecord(value)) throw new Error("The local JSON must be an object.");
    return value as T;
  } catch {
    finding(
      context,
      provider,
      area,
      "unavailable",
      "invalid-local-contract",
      `${id}: the reviewed local JSON is invalid.`,
    );
    return undefined;
  }
}

function compareVersions(left: string, right: string): number {
  const parse = (value: string) => {
    const match = /^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/u.exec(value);
    if (!match) throw new SourceShapeError("Invalid version.");
    return match.slice(1, 4).map(Number);
  };
  const leftParts = parse(left);
  const rightParts = parse(right);
  for (let index = 0; index < 3; index += 1) {
    const difference = (leftParts[index] ?? 0) - (rightParts[index] ?? 0);
    if (difference !== 0) return Math.sign(difference);
  }
  return 0;
}

function codexParserRange(source: string): {
  maximum: number;
  minimum: number;
} {
  const read = (name: string) => {
    const match = new RegExp(`const\\s+${name}:\\s*u16\\s*=\\s*(\\d+)`, "u").exec(source);
    if (!match?.[1]) throw new Error("The Codex parser range is unavailable.");
    return Number(match[1]);
  };
  return {
    maximum: read("MAX_SUPPORTED_CODEX_CLI_MINOR"),
    minimum: read("MIN_SUPPORTED_CODEX_CLI_MINOR"),
  };
}

function codexVersionIsSupported(
  version: string,
  range: { maximum: number; minimum: number },
): boolean {
  const match = /^0\.(\d+)\.[0-9A-Za-z.+-]+$/u.exec(version);
  if (!match?.[1]) return false;
  const minor = Number(match[1]);
  return minor >= range.minimum && minor <= range.maximum;
}

function claudeParserVersions(source: string): string[] {
  const match = /const\s+SUPPORTED_CLAUDE_CODE_VERSIONS:[^=]+=\s*\[([^\]]+)\]/su.exec(source);
  if (!match?.[1]) throw new Error("The Claude parser versions are unavailable.");
  const versions = [...match[1].matchAll(/"([^"]+)"/gu)].map((entry) => entry[1] ?? "");
  if (versions.length === 0 || versions.some((version) => !version)) {
    throw new Error("The Claude parser versions are invalid.");
  }
  return versions;
}

function semanticJsonValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(semanticJsonValue);
  if (!isRecord(value)) return value;
  return Object.fromEntries(
    Object.keys(value)
      .sort()
      .map((key) => [key, semanticJsonValue(value[key])]),
  );
}

export function semanticJsonSha256(value: unknown): string {
  return createHash("sha256")
    .update(JSON.stringify(semanticJsonValue(value)))
    .digest("hex");
}

function sha256Text(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

function rustWithoutComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//gu, "").replace(/^\s*\/\/.*$/gmu, "");
}

function balancedBlock(
  source: string,
  marker: string,
): { bodyStart: number; end: number; markerStart: number } {
  const markerIndex = source.indexOf(marker);
  const start = source.indexOf("{", markerIndex + marker.length);
  if (markerIndex < 0 || start < 0) {
    throw new SourceShapeError(`The ${marker} block is absent.`);
  }
  let depth = 0;
  for (let index = start; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] === "}") {
      depth -= 1;
      if (depth === 0) {
        return { bodyStart: start + 1, end: index, markerStart: markerIndex };
      }
    }
  }
  throw new SourceShapeError(`The ${marker} block is incomplete.`);
}

function rustBlock(source: string, marker: string): string {
  const block = balancedBlock(source, marker);
  return source.slice(block.markerStart, block.end + 1);
}

export function semanticCodexRustAnchorSha256(
  source: string,
  kind: ReviewedCodexRustAnchor["kind"],
): string {
  const stripped = rustWithoutComments(source);
  if (kind === "event-msg-variants") {
    const block = rustBlock(stripped, "pub enum EventMsg");
    const variants = [...block.matchAll(/^ {4}([A-Z][A-Za-z0-9_]*)\b/gmu)]
      .map((match) => match[1] ?? "")
      .filter(Boolean)
      .sort();
    if (
      variants.length === 0 ||
      variants.length > 512 ||
      !variants.includes("TokenCount") ||
      new Set(variants).size !== variants.length
    ) {
      throw new SourceShapeError("The Codex event variants are invalid.");
    }
    return sha256Text(block.replace(/\s+/gu, ""));
  }

  const mapping = rustBlock(stripped, "impl From<ResponseCompletedUsage> for TokenUsage");
  const fixture = rustBlock(stripped, "fn parses_cache_write_token_usage()");
  const compact = `${mapping}\n${fixture}`.replace(/\s+/gu, "");
  const requiredMappings = [
    "input_tokens:val.input_tokens",
    "cached_input_tokens:input_tokens_details.cached_tokens",
    "cache_write_input_tokens:input_tokens_details.cache_write_tokens",
    "output_tokens:val.output_tokens",
    "reasoning_output_tokens:val.output_tokens_details",
    "total_tokens:val.total_tokens",
    '"input_tokens":100',
    '"cached_tokens":40',
    '"cache_write_tokens":60',
    '"output_tokens":10',
    '"reasoning_tokens":5',
    '"total_tokens":110',
  ];
  if (requiredMappings.some((marker) => !compact.includes(marker))) {
    throw new SourceShapeError("The Codex token mapping changed.");
  }
  return sha256Text(compact);
}

function interfaceSignatures(source: string, name: string): Record<string, string> {
  const block = balancedBlock(source, `export interface ${name}`);
  const body = source.slice(block.bodyStart, block.end).replace(/\/\*[\s\S]*?\*\//gu, "");
  const fields = [...body.matchAll(/^\s{2}([A-Za-z_][A-Za-z0-9_]*)(\?)?:\s*([^;\r\n]+);/gmu)].map(
    (match) => {
      const field = match[1] ?? "";
      const fieldType = (match[3] ?? "").replace(/\s+/gu, " ").trim();
      return [field, `${match[2] === "?" ? "optional" : "required"}:${fieldType}`] as const;
    },
  );
  if (
    fields.length === 0 ||
    fields.length > 512 ||
    fields.some(
      ([field, signature]) =>
        !safeIdentifier(field, 128) ||
        signature.length > 256 ||
        /[\u0000-\u001f\u007f]/u.test(signature),
    ) ||
    new Set(fields.map(([field]) => field)).size !== fields.length
  )
    throw new SourceShapeError(`The ${name} interface has no fields.`);
  return Object.fromEntries(fields.sort(([left], [right]) => left.localeCompare(right)));
}

function markdownTableAfter(source: string, heading: string): string[][] {
  const headingIndex = source.indexOf(heading);
  if (headingIndex < 0) throw new SourceShapeError(`${heading} is absent.`);
  const lines = source.slice(headingIndex + heading.length).split(/\r?\n/u);
  const firstRow = lines.findIndex((line) => line.trimStart().startsWith("|"));
  if (firstRow < 0) throw new SourceShapeError(`${heading} has no table.`);
  const contiguous: string[] = [];
  for (const line of lines.slice(firstRow)) {
    if (!line.trimStart().startsWith("|")) break;
    contiguous.push(line);
  }
  const rows = contiguous.map((line) =>
    line
      .trim()
      .replace(/^\|/u, "")
      .replace(/\|$/u, "")
      .split("|")
      .map((cell) => cell.trim()),
  );
  const contentRows = rows.filter(
    (row, index) => index !== 1 || !row.every((cell) => /^:?-{3,}:?$/u.test(cell)),
  );
  if (contentRows.length < 2 || contentRows.length > 1_001)
    throw new SourceShapeError(`${heading} has no data rows.`);
  return contentRows;
}

function plainMarkdown(value: string): string {
  return value
    .replace(/\[([^\]]+)\]\([^\s)]+\)/gu, "$1")
    .replace(/[`*_]/gu, "")
    .replace(/\s+/gu, " ")
    .trim();
}

export function semanticMarkdownSha256(value: string): string {
  return sha256Text(plainMarkdown(value).toLowerCase());
}

type MarkdownSectionRange = {
  end: number;
  source: string;
  start: number;
};

function normalizedMarkdownSource(source: string): string {
  return source.replace(/\r\n/gu, "\n");
}

function indexedMarkdownLines(source: string) {
  const normalizedSource = normalizedMarkdownSource(source);
  const lines = normalizedSource.split("\n");
  const starts: number[] = [];
  let offset = 0;
  for (const line of lines) {
    starts.push(offset);
    offset += line.length + 1;
  }
  return { lines, normalizedSource, starts };
}

function markdownSectionRange(
  source: string,
  heading: string,
  selector: string,
): MarkdownSectionRange {
  const { lines, normalizedSource, starts } = indexedMarkdownLines(source);

  const candidates: MarkdownSectionRange[] = [];
  for (let index = 0; index < lines.length; index += 1) {
    if (lines[index]?.trim() !== heading) continue;
    const start = starts[index] ?? 0;
    let end = normalizedSource.length;
    for (let next = index + 1; next < lines.length; next += 1) {
      if (/^###\s+\S/u.test(lines[next]?.trim() ?? "")) {
        end = starts[next] ?? normalizedSource.length;
        break;
      }
    }
    const sectionSource = normalizedSource.slice(start, end).trimEnd();
    if (
      plainMarkdown(sectionSource).toLowerCase().includes(plainMarkdown(selector).toLowerCase())
    ) {
      candidates.push({ end, source: sectionSource, start });
    }
  }
  if (candidates.length !== 1) {
    throw new SourceShapeError(`The ${heading} pricing evidence section is not unique.`);
  }
  return candidates[0] as MarkdownSectionRange;
}

export function semanticPricingRuleWindowSha256(
  source: string,
  startHeading: string,
  endHeading: string,
): string {
  const { lines, normalizedSource, starts } = indexedMarkdownLines(source);
  const startIndexes = lines
    .map((line, index) => (line.trim() === startHeading ? index : -1))
    .filter((index) => index >= 0);
  if (startIndexes.length !== 1) {
    throw new SourceShapeError(`The ${startHeading} pricing rule window is not unique.`);
  }
  const startIndex = startIndexes[0] as number;
  const endIndex = lines.findIndex(
    (line, index) => index > startIndex && line.trim() === endHeading,
  );
  if (endIndex < 0) {
    throw new SourceShapeError(`The ${endHeading} pricing rule boundary is absent.`);
  }
  const start = starts[startIndex] ?? 0;
  const end = starts[endIndex] ?? normalizedSource.length;
  return semanticMarkdownSha256(normalizedSource.slice(start, end));
}

const monthNumbers: ReadonlyMap<string, number> = new Map([
  ["jan", 1],
  ["january", 1],
  ["feb", 2],
  ["february", 2],
  ["mar", 3],
  ["march", 3],
  ["apr", 4],
  ["april", 4],
  ["may", 5],
  ["jun", 6],
  ["june", 6],
  ["jul", 7],
  ["july", 7],
  ["aug", 8],
  ["august", 8],
  ["sep", 9],
  ["september", 9],
  ["oct", 10],
  ["october", 10],
  ["nov", 11],
  ["november", 11],
  ["dec", 12],
  ["december", 12],
] as const);

function isoDateFromParts(year: string, monthName: string, day: string): string | null {
  const month = monthNumbers.get(monthName.toLowerCase());
  if (!month) return null;
  const value = `${year}-${String(month).padStart(2, "0")}-${day.padStart(2, "0")}`;
  const parsed = new Date(`${value}T00:00:00.000Z`);
  return Number.isFinite(parsed.getTime()) && parsed.toISOString().slice(0, 10) === value
    ? value
    : null;
}

function markdownSectionDate(
  source: string,
  section: MarkdownSectionRange,
  heading: string,
): string {
  const fullDate = /^###\s+([A-Za-z]+)\s+(\d{1,2})(?:st|nd|rd|th)?,\s+(\d{4})$/u.exec(heading);
  if (fullDate?.[1] && fullDate[2] && fullDate[3]) {
    const value = isoDateFromParts(fullDate[3], fullDate[1], fullDate[2]);
    if (value) return value;
  }

  const shortDate = /^###\s+([A-Za-z]{3})\s+(\d{1,2})$/u.exec(heading);
  const monthHeading = [
    ...normalizedMarkdownSource(source)
      .slice(0, section.start)
      .matchAll(/^##\s+([A-Za-z]+),\s+(\d{4})$/gmu),
  ].at(-1);
  if (shortDate?.[1] && shortDate[2] && monthHeading?.[1] && monthHeading[2]) {
    if (
      monthNumbers.get(shortDate[1].toLowerCase()) !==
      monthNumbers.get(monthHeading[1].toLowerCase())
    ) {
      throw new SourceShapeError("The pricing evidence month heading changed.");
    }
    const value = isoDateFromParts(monthHeading[2], shortDate[1], shortDate[2]);
    if (value) return value;
  }
  throw new SourceShapeError(`The ${heading} pricing evidence date is invalid.`);
}

export function semanticPricingEvidenceSectionSha256(
  source: string,
  heading: string,
  selector: string,
): string {
  return semanticMarkdownSha256(markdownSectionRange(source, heading, selector).source);
}

export function semanticPricingEvidenceWindowSha256(
  source: string,
  sections: Array<{ heading: string; selector: string }>,
): string {
  const normalizedSource = normalizedMarkdownSource(source);
  const firstHeading = /^###\s+\S/gmu.exec(normalizedSource);
  if (!firstHeading || sections.length === 0) {
    throw new SourceShapeError("The pricing evidence window is absent.");
  }
  for (const section of sections) {
    markdownSectionRange(normalizedSource, section.heading, section.selector);
  }
  return semanticMarkdownSha256(normalizedSource.slice(firstHeading.index));
}

function parseUsdRateCell(value: string): number | null {
  const normalized = plainMarkdown(value).replace(/\s*\/\s*M(?:Tok| tokens?).*$/iu, "");
  if (normalized === "-") return null;
  const match = /^\$([0-9]+(?:\.[0-9]+)?)$/u.exec(normalized);
  if (!match?.[1]) throw new SourceShapeError("A price cell is invalid.");
  return Number(match[1]);
}

function openAiRates(
  source: string,
  heading: "### Fast pricing data" | "### Standard pricing data",
): Map<string, PublishedOpenAiRate> {
  const rows = markdownTableAfter(source, heading);
  if (rows[0]?.length !== 9) throw new SourceShapeError("The OpenAI price columns changed.");
  const rates = new Map<string, PublishedOpenAiRate>();
  for (const row of rows.slice(1)) {
    if (row.length !== 9) throw new SourceShapeError("An OpenAI price row changed.");
    const publishedModel = plainMarkdown(row[0] ?? "");
    const model = publishedModel.replace(/\s+\(<.*$/u, "");
    if (!safeIdentifier(model, 128) || rates.has(model)) {
      throw new SourceShapeError("An OpenAI model identifier is invalid.");
    }
    rates.set(model, {
      contextQualified: publishedModel !== model,
      input: parseUsdRateCell(row[1] ?? ""),
      cachedInput: parseUsdRateCell(row[2] ?? ""),
      cacheWrite: parseUsdRateCell(row[3] ?? ""),
      output: parseUsdRateCell(row[4] ?? ""),
      longInput: parseUsdRateCell(row[5] ?? ""),
      longCachedInput: parseUsdRateCell(row[6] ?? ""),
      longCacheWrite: parseUsdRateCell(row[7] ?? ""),
      longOutput: parseUsdRateCell(row[8] ?? ""),
    });
  }
  return rates;
}

function anthropicRates(source: string): Map<string, AnthropicRate & { retired: boolean }> {
  const rows = markdownTableAfter(source, "## Model pricing");
  if (rows[0]?.length !== 6) throw new SourceShapeError("The Anthropic price columns changed.");
  const rates = new Map<string, AnthropicRate & { retired: boolean }>();
  for (const row of rows.slice(1)) {
    if (row.length !== 6) throw new SourceShapeError("An Anthropic price row changed.");
    const rawName = plainMarkdown(row[0] ?? "");
    const name = rawName
      .replace(/\s+\([^)]*\)\s*$/u, "")
      .trim()
      .toLowerCase();
    if (!/^claude [a-z0-9._ -]{1,120}$/u.test(name) || rates.has(name)) {
      throw new SourceShapeError("An Anthropic model name is invalid.");
    }
    const values = row.slice(1).map((cell) => parseUsdRateCell(cell));
    if (values.some((value) => value === null)) {
      throw new SourceShapeError("An Anthropic model price is absent.");
    }
    rates.set(name, {
      input: values[0] as number,
      cacheWrite5m: values[1] as number,
      cacheWrite1h: values[2] as number,
      cacheRead: values[3] as number,
      output: values[4] as number,
      retired: /\bretired\b/iu.test(rawName),
    });
  }
  return rates;
}

function anthropicFastRates(source: string): Map<string, { input: number; output: number }> {
  const rows = markdownTableAfter(source, "### Fast mode pricing");
  if (rows[0]?.length !== 3)
    throw new SourceShapeError("The Anthropic Fast price columns changed.");
  const rates = new Map<string, { input: number; output: number }>();
  for (const row of rows.slice(1)) {
    if (row.length !== 3) throw new SourceShapeError("An Anthropic Fast price row changed.");
    const input = parseUsdRateCell(row[1] ?? "");
    const output = parseUsdRateCell(row[2] ?? "");
    if (input === null || output === null) {
      throw new SourceShapeError("An Anthropic Fast price is absent.");
    }
    for (const rawName of plainMarkdown(row[0] ?? "").split(/\s*\/\s*/u)) {
      const name = rawName.toLowerCase();
      if (!/^claude [a-z0-9._ -]{1,120}$/u.test(name) || rates.has(name)) {
        throw new SourceShapeError("An Anthropic Fast model name is invalid.");
      }
      rates.set(name, { input, output });
    }
  }
  return rates;
}

function isoDay(date: Date): string {
  return date.toISOString().slice(0, 10);
}

function dayNumber(value: string): number {
  if (!isIsoDayValue(value)) throw new Error("A reviewed date is invalid.");
  const timestamp = Date.parse(`${value}T00:00:00Z`);
  return Math.floor(timestamp / 86_400_000);
}

function currentPeriod<T extends { effectiveFrom: string; effectiveUntil: string | null }>(
  periods: T[],
  day: string,
): T | undefined {
  return periods.find(
    (period) =>
      period.effectiveFrom <= day &&
      (period.effectiveUntil === null || day < period.effectiveUntil),
  );
}

function upcomingPeriod<T extends { effectiveFrom: string }>(
  periods: T[],
  day: string,
): T | undefined {
  return [...periods]
    .filter((period) => period.effectiveFrom > day)
    .sort((left, right) => left.effectiveFrom.localeCompare(right.effectiveFrom))[0];
}

function ratesEqual(left: number | null, right: number | null): boolean {
  if (left === null || right === null) return left === right;
  return Math.abs(left - right) < 1e-9;
}

function formatNullableRate(value: number | null): string {
  return value === null ? "none" : String(value);
}

function anthropicDisplayName(model: string): string | null {
  const withoutDate = model.replace(/-\d{8}$/u, "");
  let match = /^claude-(fable|mythos|opus|sonnet|haiku)-([0-9]+(?:-[0-9]+)*)$/u.exec(withoutDate);
  if (match?.[1] && match[2]) {
    return `claude ${match[1]} ${match[2].replaceAll("-", ".")}`;
  }
  match = /^claude-([0-9]+)-([0-9]+)-(opus|sonnet|haiku)$/u.exec(withoutDate);
  if (match?.[1] && match[2] && match[3]) {
    return `claude ${match[3]} ${match[1]}.${match[2]}`;
  }
  return null;
}

function reviewedModelNames(model: AnthropicManifestModel): string[] {
  return [model.name, ...model.aliases]
    .map(anthropicDisplayName)
    .filter((name): name is string => name !== null);
}

function supportsUsInference(name: string): boolean {
  const match = /^claude (?:fable|mythos|opus|sonnet|haiku) (\d+)(?:\.(\d+))?/u.exec(name);
  if (!match?.[1]) return false;
  const major = Number(match[1]);
  const minor = Number(match[2] ?? 0);
  return major > 4 || (major === 4 && minor >= 6);
}

function validateManifestShape(manifest: OpenAiManifest | AnthropicManifest, provider: Provider) {
  if (
    manifest.schemaVersion !== 1 ||
    !safeIdentifier(manifest.basis, 128) ||
    !Array.isArray(manifest.models) ||
    manifest.models.length === 0 ||
    manifest.models.length > 1_000
  ) {
    throw new Error(`The ${provider} pricing manifest is invalid.`);
  }
}

function auditManifestCheckpoint(
  context: AuditContext,
  provider: Provider,
  manifest: OpenAiManifest | AnthropicManifest,
  expectedSha256: string,
) {
  if (semanticJsonSha256(manifest) !== expectedSha256) {
    finding(
      context,
      provider,
      "pricing",
      "review-required",
      "reviewed-snapshot-changed",
      `The bundled ${provider === "codex" ? "OpenAI" : "Anthropic"} pricing manifest differs from the reviewed snapshot.`,
    );
  }
}

type ManifestPeriod = {
  effectiveFrom: string;
  effectiveUntil: string | null;
};

type ManifestPeriodGroup = {
  model: string;
  periodKind: PricingPeriodKind;
  periods: ManifestPeriod[];
};

function pricingBoundaryKey(
  model: string,
  periodKind: PricingPeriodKind,
  boundary: "end" | "start",
  date: string,
): string {
  return `${model}|${periodKind}|${boundary}|${date}`;
}

function checkedManifestPeriods(value: unknown): ManifestPeriod[] {
  if (!Array.isArray(value) || value.length > 1_000) {
    throw new Error("A pricing period list is invalid.");
  }
  return value.map((period) => {
    if (
      !isRecord(period) ||
      !isIsoDayValue(period.effectiveFrom) ||
      !(period.effectiveUntil === null || isIsoDayValue(period.effectiveUntil)) ||
      (typeof period.effectiveUntil === "string" && period.effectiveUntil <= period.effectiveFrom)
    ) {
      throw new Error("A pricing period is invalid.");
    }
    return {
      effectiveFrom: period.effectiveFrom,
      effectiveUntil: period.effectiveUntil as string | null,
    };
  });
}

function manifestPeriodGroups(
  provider: Provider,
  manifest: OpenAiManifest | AnthropicManifest,
): Map<string, ManifestPeriodGroup> {
  const groups = new Map<string, ManifestPeriodGroup>();
  for (const rawModel of manifest.models as unknown[]) {
    if (!isRecord(rawModel)) throw new Error("A pricing model is invalid.");
    const model = safeIdentifier(rawModel.name, 128);
    if (!model) throw new Error("A pricing model identifier is invalid.");
    const add = (periodKind: PricingPeriodKind, periods: ManifestPeriod[]) => {
      const key = `${model}|${periodKind}`;
      if (groups.has(key)) throw new Error("A pricing model is duplicated.");
      groups.set(key, { model, periodKind, periods });
    };
    if (provider === "codex") {
      const periods = checkedManifestPeriods(rawModel.periods);
      const rawPeriods = rawModel.periods as unknown[];
      const fastLongContext = rawPeriods.flatMap((period) => {
        if (!isRecord(period)) throw new Error("An OpenAI pricing period is invalid.");
        return period.fastLongContext === undefined
          ? []
          : checkedManifestPeriods([period.fastLongContext]);
      });
      add("standard", periods);
      add("fast-long-context", fastLongContext);
    } else {
      add("standard", checkedManifestPeriods(rawModel.standardPeriods));
      add("fast", checkedManifestPeriods(rawModel.fastPeriods));
    }
  }
  return groups;
}

function manifestPricingBoundaryKeys(groups: Map<string, ManifestPeriodGroup>): Set<string> {
  const keys = new Set<string>();
  for (const group of groups.values()) {
    for (const period of group.periods) {
      const start = pricingBoundaryKey(
        group.model,
        group.periodKind,
        "start",
        period.effectiveFrom,
      );
      if (keys.has(start)) throw new Error("A pricing period boundary is duplicated.");
      keys.add(start);
      if (period.effectiveUntil !== null) {
        const end = pricingBoundaryKey(group.model, group.periodKind, "end", period.effectiveUntil);
        if (keys.has(end)) throw new Error("A pricing period boundary is duplicated.");
        keys.add(end);
      }
    }
  }
  return keys;
}

function reviewedPricingBoundaryKeys(evidence: ReviewedPricingEvidence): Set<string> {
  const keys = new Set<string>();
  for (const source of evidence.sources) {
    for (const section of source.sections) {
      for (const checkpoint of section.checkpoints) {
        if (checkpoint.boundary === "absent") continue;
        keys.add(
          pricingBoundaryKey(
            checkpoint.model,
            checkpoint.periodKind,
            checkpoint.boundary,
            section.date,
          ),
        );
      }
    }
  }
  for (const exemption of evidence.boundaryExemptions) {
    keys.add(
      pricingBoundaryKey(exemption.model, exemption.periodKind, exemption.boundary, exemption.date),
    );
  }
  return keys;
}

async function auditPricingEvidence(
  context: AuditContext,
  provider: Provider,
  manifest: OpenAiManifest | AnthropicManifest,
  evidence: ReviewedPricingEvidence,
) {
  const periodGroups = manifestPeriodGroups(provider, manifest);
  const manifestBoundaries = manifestPricingBoundaryKeys(periodGroups);
  const reviewedBoundaries = reviewedPricingBoundaryKeys(evidence);
  const missing = [...manifestBoundaries].filter((key) => !reviewedBoundaries.has(key));
  const extra = [...reviewedBoundaries].filter((key) => !manifestBoundaries.has(key));
  if (missing.length > 0 || extra.length > 0) {
    finding(
      context,
      provider,
      "pricing",
      "review-required",
      "effective-date-changed",
      `The reviewed pricing date coverage changed: ${missing.length} manifest boundary(s) lack evidence and ${extra.length} evidence boundary(s) lack a manifest match.`,
    );
  }

  for (const evidenceSource of evidence.sources) {
    for (const section of evidenceSource.sections) {
      for (const checkpoint of section.checkpoints) {
        if (checkpoint.boundary !== "absent") continue;
        const group = periodGroups.get(`${checkpoint.model}|${checkpoint.periodKind}`);
        if (!group) throw new Error("An absent pricing checkpoint has no reviewed model.");
      }
    }
  }

  await Promise.all(
    evidence.sources.map(async (evidenceSource) => {
      const source = await fetchSource(
        {
          area: "pricing",
          context,
          id: `${provider}-${evidenceSource.id}-pricing-evidence`,
          provider,
        },
        evidenceSource.url,
      );
      if (source === undefined) return;

      try {
        const observedWindow = semanticPricingEvidenceWindowSha256(source, evidenceSource.sections);
        if (observedWindow !== evidenceSource.windowSemanticSha256) {
          finding(
            context,
            provider,
            "pricing",
            "review-required",
            "pricing-evidence-changed",
            `${evidenceSource.id}: the reviewed dated pricing evidence window changed.`,
            evidenceSource.url,
          );
        }
      } catch {
        finding(
          context,
          provider,
          "pricing",
          "review-required",
          "pricing-evidence-changed",
          `${evidenceSource.id}: the dated pricing evidence window structure changed.`,
          evidenceSource.url,
        );
        return;
      }

      for (const section of evidenceSource.sections) {
        try {
          const range = markdownSectionRange(source, section.heading, section.selector);
          const observedDate = markdownSectionDate(source, range, section.heading);
          const observedSha256 = semanticMarkdownSha256(range.source);
          if (observedDate !== section.date || observedSha256 !== section.semanticSha256) {
            finding(
              context,
              provider,
              "pricing",
              "review-required",
              "pricing-evidence-changed",
              `${evidenceSource.id}/${section.id}: the reviewed dated pricing section changed.`,
              evidenceSource.url,
            );
          }
          const normalizedSection = plainMarkdown(range.source).toLowerCase();
          for (const checkpoint of section.checkpoints) {
            const group = periodGroups.get(`${checkpoint.model}|${checkpoint.periodKind}`);
            if (!group) throw new Error("A pricing checkpoint has no reviewed model.");
            const checkpointDate =
              checkpoint.boundary === "absent" ? checkpoint.date : section.date;
            const matches =
              checkpoint.boundary === "start"
                ? group.periods.some((period) => period.effectiveFrom === checkpointDate)
                : checkpoint.boundary === "end"
                  ? group.periods.some((period) => period.effectiveUntil === checkpointDate)
                  : !group.periods.some((period) => period.effectiveFrom === checkpointDate) &&
                    normalizedSection.includes(plainMarkdown(checkpoint.marker).toLowerCase());
            if (matches) continue;
            finding(
              context,
              provider,
              "pricing",
              "review-required",
              "effective-date-changed",
              `${checkpoint.model}: the reviewed ${checkpoint.periodKind} ${checkpoint.boundary} boundary at ${checkpointDate} changed.`,
              evidenceSource.url,
            );
          }
        } catch {
          finding(
            context,
            provider,
            "pricing",
            "review-required",
            "pricing-evidence-changed",
            `${evidenceSource.id}/${section.id}: the dated pricing section structure changed.`,
            evidenceSource.url,
          );
        }
      }
    }),
  );
}

async function auditCodex(context: AuditContext) {
  const contract = context.contract.codex;
  const parserSource = readLocalSource(
    { area: "parser", context, id: "codex-parser", provider: "codex" },
    contract.parserSourcePath,
  );
  let parserRange: { maximum: number; minimum: number } | undefined;
  if (parserSource !== undefined) {
    try {
      parserRange = codexParserRange(parserSource);
    } catch {
      finding(
        context,
        "codex",
        "parser",
        "unavailable",
        "invalid-local-contract",
        "codex-parser: the supported CLI range cannot be read.",
      );
    }
  }

  const release = await fetchJson(
    { area: "parser", context, id: "codex-release", provider: "codex" },
    contract.releaseUrl,
  );
  let releaseTag: string | undefined;
  let releaseVersion: string | undefined;
  if (isRecord(release)) {
    releaseTag = safeIdentifier(release.tag_name);
    releaseVersion = releaseTag?.replace(/^rust-v/u, "");
    if (!releaseTag?.startsWith("rust-v") || !releaseVersion) {
      finding(
        context,
        "codex",
        "parser",
        "unavailable",
        "invalid-source",
        "codex-release: the release tag is invalid.",
        contract.releaseUrl,
      );
      releaseTag = undefined;
      releaseVersion = undefined;
    }
  } else if (release !== undefined) {
    finding(
      context,
      "codex",
      "parser",
      "unavailable",
      "invalid-source",
      "codex-release: the release record is invalid.",
      contract.releaseUrl,
    );
  }
  if (releaseVersion !== undefined) {
    try {
      if (compareVersions(releaseVersion, contract.reviewedRelease) !== 0) {
        finding(
          context,
          "codex",
          "parser",
          "review-required",
          "upstream-release-changed",
          `Codex ${releaseVersion} differs from reviewed release ${contract.reviewedRelease}.`,
          contract.releaseUrl,
        );
      }
      if (parserRange && !codexVersionIsSupported(releaseVersion, parserRange)) {
        finding(
          context,
          "codex",
          "parser",
          "review-required",
          "unsupported-version",
          `Codex ${releaseVersion} is outside parser range 0.${parserRange.minimum} through 0.${parserRange.maximum}.`,
          contract.releaseUrl,
        );
      }
    } catch {
      finding(
        context,
        "codex",
        "parser",
        "unavailable",
        "invalid-source",
        "codex-release: the release version is invalid.",
        contract.releaseUrl,
      );
    }
  }

  let modelCatalog: unknown;
  if (releaseTag) {
    await Promise.all([
      ...contract.schemas.map(async (schema) => {
        const url = `${contract.repositoryRawBaseUrl}/${releaseTag}/${schema.path}`;
        const value = await fetchJson(
          {
            area: "parser",
            context,
            id: `codex-${schema.id}-schema`,
            provider: "codex",
          },
          url,
        );
        if (value !== undefined) {
          if (!isRecord(value)) {
            finding(
              context,
              "codex",
              "parser",
              "unavailable",
              "invalid-source",
              `${schema.id}: the generated usage schema is invalid.`,
              url,
            );
            return;
          }
          const observed = semanticJsonSha256(value);
          if (observed !== schema.semanticSha256) {
            finding(
              context,
              "codex",
              "parser",
              "review-required",
              "schema-changed",
              `${schema.id}: the generated usage schema changed (${schema.semanticSha256.slice(0, 12)} to ${observed.slice(0, 12)}).`,
              url,
            );
          }
        }
      }),
      ...contract.rustAnchors.map(async (anchor) => {
        const url = `${contract.repositoryRawBaseUrl}/${releaseTag}/${anchor.path}`;
        const source = await fetchSource(
          {
            area: "parser",
            context,
            id: `codex-${anchor.id}-source`,
            provider: "codex",
          },
          url,
        );
        if (source === undefined) return;
        try {
          const observed = semanticCodexRustAnchorSha256(source, anchor.kind);
          if (observed !== anchor.semanticSha256) {
            finding(
              context,
              "codex",
              "parser",
              "review-required",
              anchor.kind === "event-msg-variants"
                ? "event-kind-changed"
                : "token-semantics-changed",
              `${anchor.id}: the reviewed Codex source anchor changed (${anchor.semanticSha256.slice(0, 12)} to ${observed.slice(0, 12)}).`,
              url,
            );
          }
        } catch {
          finding(
            context,
            "codex",
            "parser",
            "review-required",
            anchor.kind === "event-msg-variants" ? "event-kind-changed" : "token-semantics-changed",
            `${anchor.id}: the reviewed Codex source structure changed.`,
            url,
          );
        }
      }),
    ]);
    const catalogUrl = `${contract.repositoryRawBaseUrl}/${releaseTag}/${contract.modelCatalogPath}`;
    modelCatalog = await fetchJson(
      { area: "pricing", context, id: "codex-model-catalog", provider: "codex" },
      catalogUrl,
    );
    if (modelCatalog !== undefined && !isRecord(modelCatalog)) {
      finding(
        context,
        "codex",
        "pricing",
        "unavailable",
        "invalid-source",
        "codex-model-catalog: the model catalog is invalid.",
        catalogUrl,
      );
      modelCatalog = undefined;
    }
  }

  const pricingSourcePromise = fetchSource(
    { area: "pricing", context, id: "openai-pricing", provider: "codex" },
    contract.pricingSourceUrl,
  );
  const manifestDescriptor = {
    area: "pricing",
    context,
    id: "openai-pricing-manifest",
    provider: "codex",
  } as const;
  const manifest = parseJsonSource<OpenAiManifest>(
    manifestDescriptor,
    readLocalSource(manifestDescriptor, contract.pricingManifestPath),
  );
  const pricingSource = await pricingSourcePromise;
  let manifestIsValid = false;
  if (manifest) {
    try {
      validateManifestShape(manifest, "codex");
      auditManifestCheckpoint(context, "codex", manifest, contract.pricingManifestSemanticSha256);
      await auditPricingEvidence(context, "codex", manifest, contract.pricingEvidence);
      manifestIsValid = true;
    } catch {
      finding(
        context,
        "codex",
        "pricing",
        "unavailable",
        "invalid-local-contract",
        "The bundled OpenAI pricing manifest is invalid.",
      );
    }
  }
  if (manifestIsValid && manifest && pricingSource && isRecord(modelCatalog)) {
    try {
      auditOpenAiPricing(context, manifest, modelCatalog, pricingSource);
    } catch (error) {
      const sourceChanged = error instanceof SourceShapeError;
      finding(
        context,
        "codex",
        "pricing",
        sourceChanged ? "review-required" : "unavailable",
        sourceChanged ? "invalid-source" : "invalid-local-contract",
        sourceChanged
          ? "OpenAI pricing or model source structure changed."
          : "The bundled OpenAI pricing manifest is invalid.",
        contract.pricingSourceUrl,
      );
    }
  }
}

function auditOpenAiPricing(
  context: AuditContext,
  manifest: OpenAiManifest,
  modelCatalog: Record<string, unknown>,
  pricingSource: string,
) {
  auditPricingRuleWindows(context, "codex", pricingSource);
  const models = modelCatalog.models;
  if (
    !Array.isArray(models) ||
    models.length === 0 ||
    models.length > 1_000 ||
    !models.every(isRecord)
  ) {
    throw new SourceShapeError("The Codex model catalog is invalid.");
  }
  const officialRates = openAiRates(pricingSource, "### Standard pricing data");
  const officialFastRates = openAiRates(pricingSource, "### Fast pricing data");
  const today = isoDay(context.now);
  const normalizedPricingSource = plainMarkdown(pricingSource).toLowerCase();
  const missingSemanticMarkers = context.contract.codex.pricingSemanticMarkers.filter(
    (marker) => !normalizedPricingSource.includes(plainMarkdown(marker).toLowerCase()),
  );
  if (missingSemanticMarkers.length > 0) {
    finding(
      context,
      "codex",
      "pricing",
      "review-required",
      "pricing-modifier-changed",
      `The reviewed OpenAI pricing modifiers changed: ${missingSemanticMarkers.length} semantic marker(s) are absent.`,
      context.contract.codex.pricingSourceUrl,
    );
  }

  for (const reviewed of manifest.models) {
    const period = currentPeriod(reviewed.periods, today);
    if (!period) continue;
    const multiplier = period.fastMultiplier;
    const fastLongContext = period.fastLongContext
      ? currentPeriod([period.fastLongContext], today)
      : undefined;
    const expectedFast: OpenAiRate = {
      input: multiplier === undefined ? null : period.inputUsdPerMillion * multiplier,
      cachedInput: multiplier === undefined ? null : period.cachedInputUsdPerMillion * multiplier,
      cacheWrite:
        multiplier === undefined || period.cacheWriteUsdPerMillion === null
          ? null
          : period.cacheWriteUsdPerMillion * multiplier,
      output: multiplier === undefined ? null : period.outputUsdPerMillion * multiplier,
      longInput: fastLongContext?.inputUsdPerMillion ?? null,
      longCachedInput: fastLongContext?.cachedInputUsdPerMillion ?? null,
      longCacheWrite: fastLongContext?.cacheWriteUsdPerMillion ?? null,
      longOutput: fastLongContext?.outputUsdPerMillion ?? null,
    };
    const publishedFast = officialFastRates.get(reviewed.name);
    if (!publishedFast) {
      finding(
        context,
        "codex",
        "pricing",
        "review-required",
        "unknown-price",
        `${reviewed.name}: the official Fast price table has no row.`,
        context.contract.codex.pricingSourceUrl,
      );
      continue;
    }
    const changedFast = (Object.keys(expectedFast) as Array<keyof OpenAiRate>).filter(
      (key) => !ratesEqual(expectedFast[key], publishedFast[key]),
    );
    const usesFastAllContextFallback =
      period.fastLongContext === undefined &&
      period.fastMultiplier !== undefined &&
      period.longContext.inputMultiplier === 1 &&
      period.longContext.outputMultiplier === 1;
    if (usesFastAllContextFallback && publishedFast.contextQualified) {
      changedFast.push("longInput");
    }
    if (changedFast.length > 0) {
      finding(
        context,
        "codex",
        "pricing",
        "review-required",
        "pricing-modifier-changed",
        `${reviewed.name}: official Fast pricing differs in ${[...new Set(changedFast)].join(", ")}; bundled input/output ${formatNullableRate(expectedFast.input)}/${formatNullableRate(expectedFast.output)}, official ${formatNullableRate(publishedFast.input)}/${formatNullableRate(publishedFast.output)}.`,
        context.contract.codex.pricingSourceUrl,
      );
    }
  }

  const visibleModels = models.filter(
    (model): model is Record<string, unknown> =>
      isRecord(model) && model.visibility === "list" && model.supported_in_api === true,
  );
  if (visibleModels.length === 0) {
    throw new SourceShapeError("The Codex model catalog has no visible models.");
  }
  for (const officialModel of visibleModels) {
    const model = safeIdentifier(officialModel.slug, 128);
    if (!model) throw new SourceShapeError("A Codex model slug is invalid.");
    const reviewed = manifest.models.find(
      (entry) => entry.name === model || entry.aliases.includes(model),
    );
    if (!reviewed) {
      finding(
        context,
        "codex",
        "pricing",
        "review-required",
        "unknown-model",
        `${model}: the visible Codex model has no bundled price rule.`,
        context.contract.codex.pricingSourceUrl,
      );
      continue;
    }
    const period = currentPeriod(reviewed.periods, today);
    if (!period) {
      finding(
        context,
        "codex",
        "pricing",
        "review-required",
        "unknown-price",
        `${model}: the bundled catalog has no current price period.`,
        context.contract.codex.pricingSourceUrl,
      );
      continue;
    }
    const published = officialRates.get(model);
    if (!published) {
      finding(
        context,
        "codex",
        "pricing",
        "review-required",
        "unknown-price",
        `${model}: the official Standard price table has no row.`,
        context.contract.codex.pricingSourceUrl,
      );
      continue;
    }
    const expected: OpenAiRate = {
      input: period.inputUsdPerMillion,
      cachedInput: period.cachedInputUsdPerMillion,
      cacheWrite: period.cacheWriteUsdPerMillion,
      output: period.outputUsdPerMillion,
      longInput: period.inputUsdPerMillion * period.longContext.inputMultiplier,
      longCachedInput: period.cachedInputUsdPerMillion * period.longContext.inputMultiplier,
      longCacheWrite:
        period.cacheWriteUsdPerMillion === null
          ? null
          : period.cacheWriteUsdPerMillion * period.longContext.inputMultiplier,
      longOutput: period.outputUsdPerMillion * period.longContext.outputMultiplier,
    };
    const publishedAllContext =
      !published.contextQualified &&
      period.longContext.inputMultiplier === 1 &&
      period.longContext.outputMultiplier === 1 &&
      published.longInput === null &&
      published.longCachedInput === null &&
      published.longCacheWrite === null &&
      published.longOutput === null
        ? {
            ...published,
            longInput: published.input,
            longCachedInput: published.cachedInput,
            longCacheWrite: published.cacheWrite,
            longOutput: published.output,
          }
        : published;
    const changed = (Object.keys(expected) as Array<keyof OpenAiRate>).filter(
      (key) => !ratesEqual(expected[key], publishedAllContext[key]),
    );
    const contextWindow = officialModel.context_window;
    if (
      typeof contextWindow !== "number" ||
      !Number.isSafeInteger(contextWindow) ||
      contextWindow <= 0
    ) {
      throw new SourceShapeError("A Codex context window is invalid.");
    }
    if (period.longContext.inputTokensAbove !== contextWindow) {
      changed.push("longInput");
    }
    if (changed.length > 0) {
      finding(
        context,
        "codex",
        "pricing",
        "review-required",
        "price-changed",
        `${model}: official Standard pricing differs in ${[...new Set(changed)].join(", ")}; bundled input/output ${formatNullableRate(expected.input)}/${formatNullableRate(expected.output)}, official ${formatNullableRate(published.input)}/${formatNullableRate(published.output)}.`,
        context.contract.codex.pricingSourceUrl,
      );
    }
  }
}

function auditPricingRuleWindows(context: AuditContext, provider: Provider, source: string) {
  const contract = context.contract[provider];
  for (const window of contract.pricingRuleWindows) {
    try {
      const observed = semanticPricingRuleWindowSha256(
        source,
        window.startHeading,
        window.endHeading,
      );
      if (observed === window.semanticSha256) continue;
      finding(
        context,
        provider,
        "pricing",
        "review-required",
        "pricing-modifier-changed",
        `${window.id}: the reviewed pricing rule window changed (${window.semanticSha256.slice(0, 12)} to ${observed.slice(0, 12)}).`,
        contract.pricingSourceUrl,
      );
    } catch {
      finding(
        context,
        provider,
        "pricing",
        "review-required",
        "pricing-modifier-changed",
        `${window.id}: the reviewed pricing rule window structure changed.`,
        contract.pricingSourceUrl,
      );
    }
  }
}

async function auditClaudeTokenSemantics(context: AuditContext, usageTypes: string | undefined) {
  for (const semanticSource of context.contract.claude.usageSemanticSources) {
    const source =
      semanticSource.url === context.contract.claude.usageTypesSourceUrl
        ? usageTypes
        : await fetchSource(
            {
              area: "parser",
              context,
              id: `claude-${semanticSource.id}-semantics`,
              provider: "claude",
            },
            semanticSource.url,
          );
    if (source === undefined) continue;
    const normalized = plainMarkdown(source).toLowerCase();
    const missing = semanticSource.markers.some(
      (marker) => !normalized.includes(plainMarkdown(marker).toLowerCase()),
    );
    if (missing) {
      finding(
        context,
        "claude",
        "parser",
        "review-required",
        "token-semantics-changed",
        `${semanticSource.id}: the reviewed Claude token meaning changed.`,
        semanticSource.url,
      );
    }
  }
}

async function auditClaude(context: AuditContext) {
  const contract = context.contract.claude;
  const parserSource = readLocalSource(
    { area: "parser", context, id: "claude-parser", provider: "claude" },
    contract.parserSourcePath,
  );
  let supportedVersions: string[] | undefined;
  if (parserSource !== undefined) {
    try {
      supportedVersions = claudeParserVersions(parserSource);
    } catch {
      finding(
        context,
        "claude",
        "parser",
        "unavailable",
        "invalid-local-contract",
        "claude-parser: the exact version allow-list cannot be read.",
      );
    }
  }

  const versionSources = [
    ["stable", contract.stablePackageUrl, contract.reviewedStableVersion],
    ["latest", contract.latestPackageUrl, contract.reviewedLatestVersion],
  ] as const;
  await Promise.all(
    versionSources.map(async ([channel, url, reviewed]) => {
      const value = await fetchJson(
        {
          area: "parser",
          context,
          id: `claude-code-${channel}`,
          provider: "claude",
        },
        url,
      );
      const version = isRecord(value) ? safeIdentifier(value.version) : null;
      if (!version) {
        if (value !== undefined) {
          finding(
            context,
            "claude",
            "parser",
            "unavailable",
            "invalid-source",
            `claude-code-${channel}: the package version is invalid.`,
            url,
          );
        }
        return;
      }
      try {
        if (compareVersions(version, reviewed) !== 0) {
          finding(
            context,
            "claude",
            "parser",
            "review-required",
            "upstream-release-changed",
            `Claude Code ${channel} ${version} differs from reviewed version ${reviewed}.`,
            url,
          );
        }
      } catch {
        finding(
          context,
          "claude",
          "parser",
          "unavailable",
          "invalid-source",
          `claude-code-${channel}: the package version is invalid.`,
          url,
        );
      }
      if (supportedVersions && !supportedVersions.includes(version)) {
        finding(
          context,
          "claude",
          "parser",
          "review-required",
          "unsupported-version",
          `Claude Code ${channel} ${version} is outside the exact transcript allow-list.`,
          url,
        );
      }
    }),
  );

  const [agentSdk, usageTypes, pricingSource] = await Promise.all([
    fetchJson(
      { area: "parser", context, id: "claude-agent-sdk", provider: "claude" },
      contract.agentSdkPackageUrl,
    ),
    fetchSource(
      { area: "parser", context, id: "anthropic-usage-types", provider: "claude" },
      contract.usageTypesSourceUrl,
    ),
    fetchSource(
      { area: "pricing", context, id: "anthropic-pricing", provider: "claude" },
      contract.pricingSourceUrl,
    ),
  ]);
  if (isRecord(agentSdk)) {
    const version = safeIdentifier(agentSdk.version);
    if (!version) {
      finding(
        context,
        "claude",
        "parser",
        "unavailable",
        "invalid-source",
        "claude-agent-sdk: the package version is invalid.",
        contract.agentSdkPackageUrl,
      );
    } else if (version !== contract.reviewedAgentSdkVersion) {
      finding(
        context,
        "claude",
        "parser",
        "review-required",
        "upstream-release-changed",
        `Claude Agent SDK ${version} differs from reviewed version ${contract.reviewedAgentSdkVersion}.`,
        contract.agentSdkPackageUrl,
      );
    }
  } else if (agentSdk !== undefined) {
    finding(
      context,
      "claude",
      "parser",
      "unavailable",
      "invalid-source",
      "claude-agent-sdk: the package record is invalid.",
      contract.agentSdkPackageUrl,
    );
  }
  if (usageTypes !== undefined) {
    try {
      for (const [name, reviewedFields] of Object.entries(contract.usageInterfaces)) {
        const observedFields = interfaceSignatures(usageTypes, name);
        const reviewedNames = Object.keys(reviewedFields).sort();
        const observedNames = Object.keys(observedFields).sort();
        const added = observedNames.filter((field) => !(field in reviewedFields));
        const removed = reviewedNames.filter((field) => !(field in observedFields));
        const modified = observedNames.filter(
          (field) => field in reviewedFields && observedFields[field] !== reviewedFields[field],
        );
        const changed = [
          ...added.map((field) => `+${field}`),
          ...removed.map((field) => `-${field}`),
          ...modified.map((field) => `~${field}`),
        ];
        if (changed.length > 0) {
          finding(
            context,
            "claude",
            "parser",
            "review-required",
            "unknown-field",
            `${name}: the reviewed usage fields changed (${changed.slice(0, 12).join(", ") || "field set changed"}).`,
            contract.usageTypesSourceUrl,
          );
        }
      }
    } catch {
      finding(
        context,
        "claude",
        "parser",
        "review-required",
        "invalid-source",
        "The Anthropic usage type structure changed.",
        contract.usageTypesSourceUrl,
      );
    }
  }
  await auditClaudeTokenSemantics(context, usageTypes);

  const manifestDescriptor = {
    area: "pricing",
    context,
    id: "anthropic-pricing-manifest",
    provider: "claude",
  } as const;
  const manifest = parseJsonSource<AnthropicManifest>(
    manifestDescriptor,
    readLocalSource(manifestDescriptor, contract.pricingManifestPath),
  );
  let manifestIsValid = false;
  if (manifest) {
    try {
      validateManifestShape(manifest, "claude");
      auditManifestCheckpoint(context, "claude", manifest, contract.pricingManifestSemanticSha256);
      await auditPricingEvidence(context, "claude", manifest, contract.pricingEvidence);
      manifestIsValid = true;
    } catch {
      finding(
        context,
        "claude",
        "pricing",
        "unavailable",
        "invalid-local-contract",
        "The bundled Anthropic pricing manifest is invalid.",
      );
    }
  }
  if (manifestIsValid && manifest && pricingSource) {
    try {
      auditAnthropicPricing(context, manifest, pricingSource);
    } catch (error) {
      const sourceChanged = error instanceof SourceShapeError;
      finding(
        context,
        "claude",
        "pricing",
        sourceChanged ? "review-required" : "unavailable",
        sourceChanged ? "invalid-source" : "invalid-local-contract",
        sourceChanged
          ? "Anthropic pricing source structure changed."
          : "The bundled Anthropic pricing manifest is invalid.",
        contract.pricingSourceUrl,
      );
    }
  }
}

function auditAnthropicPricing(context: AuditContext, manifest: AnthropicManifest, source: string) {
  auditPricingRuleWindows(context, "claude", source);
  const official = anthropicRates(source);
  const officialFast = anthropicFastRates(source);
  const today = isoDay(context.now);
  for (const [name, published] of official) {
    if (published.retired) continue;
    const reviewed = manifest.models.find((model) => reviewedModelNames(model).includes(name));
    if (!reviewed) {
      finding(
        context,
        "claude",
        "pricing",
        "review-required",
        "unknown-model",
        `${name}: the current Claude model has no bundled price rule.`,
        context.contract.claude.pricingSourceUrl,
      );
    }
  }
  for (const name of officialFast.keys()) {
    const reviewed = manifest.models.find((model) => reviewedModelNames(model).includes(name));
    if (!reviewed || !currentPeriod(reviewed.fastPeriods, today)) {
      finding(
        context,
        "claude",
        "pricing",
        "review-required",
        "pricing-modifier-changed",
        `${name}: official Fast support has no current bundled rule.`,
        context.contract.claude.pricingSourceUrl,
      );
    }
  }
  for (const model of manifest.models) {
    const names = reviewedModelNames(model);
    const expectedUsInference = names.some(supportsUsInference);
    if (model.supportsUsInference !== expectedUsInference) {
      finding(
        context,
        "claude",
        "pricing",
        "review-required",
        "pricing-modifier-changed",
        `${model.name}: bundled US inference support differs from the documented Claude 4.6 and later rule.`,
        context.contract.claude.pricingSourceUrl,
      );
    }
    const name = names.find((candidate) => official.has(candidate));
    const published = name ? official.get(name) : undefined;
    const standard = currentPeriod(model.standardPeriods, today);
    if (!standard) {
      if (published && !published.retired) {
        finding(
          context,
          "claude",
          "pricing",
          "review-required",
          "unknown-price",
          `${model.name}: the bundled catalog has no current price period.`,
          context.contract.claude.pricingSourceUrl,
        );
      }
    } else {
      if (!published || published.retired) {
        finding(
          context,
          "claude",
          "pricing",
          "review-required",
          "unknown-price",
          `${model.name}: the official current model price is absent.`,
          context.contract.claude.pricingSourceUrl,
        );
      } else {
        const changed = [
          ["input", standard.inputUsdPerMillion, published.input],
          ["cacheWrite5m", standard.cacheWrite5mUsdPerMillion, published.cacheWrite5m],
          ["cacheWrite1h", standard.cacheWrite1hUsdPerMillion, published.cacheWrite1h],
          ["cacheRead", standard.cacheReadUsdPerMillion, published.cacheRead],
          ["output", standard.outputUsdPerMillion, published.output],
        ].filter(([, reviewed, current]) => !ratesEqual(reviewed as number, current as number));
        if (changed.length > 0) {
          finding(
            context,
            "claude",
            "pricing",
            "review-required",
            "price-changed",
            `${model.name}: official standard pricing differs in ${changed.map(([field]) => field).join(", ")}; bundled input/output ${standard.inputUsdPerMillion}/${standard.outputUsdPerMillion}, official ${published.input}/${published.output}.`,
            context.contract.claude.pricingSourceUrl,
          );
        }
      }
    }

    const upcoming = upcomingPeriod(model.standardPeriods, today);
    if (
      upcoming &&
      dayNumber(upcoming.effectiveFrom) - dayNumber(today) <= context.contract.reviewEveryDays &&
      name
    ) {
      if (
        published &&
        (!ratesEqual(upcoming.inputUsdPerMillion, published.input) ||
          !ratesEqual(upcoming.cacheWrite5mUsdPerMillion, published.cacheWrite5m) ||
          !ratesEqual(upcoming.cacheWrite1hUsdPerMillion, published.cacheWrite1h) ||
          !ratesEqual(upcoming.cacheReadUsdPerMillion, published.cacheRead) ||
          !ratesEqual(upcoming.outputUsdPerMillion, published.output))
      ) {
        finding(
          context,
          "claude",
          "pricing",
          "review-required",
          "future-price-changed",
          `${model.name}: the ${upcoming.effectiveFrom} bundled price differs from the current official price.`,
          context.contract.claude.pricingSourceUrl,
        );
      }
    }

    const fast = currentPeriod(model.fastPeriods, today);
    if (fast) {
      const fastName = names.find((candidate) => officialFast.has(candidate));
      const published = fastName ? officialFast.get(fastName) : undefined;
      if (
        !published ||
        !ratesEqual(fast.inputUsdPerMillion, published.input) ||
        !ratesEqual(fast.outputUsdPerMillion, published.output) ||
        !ratesEqual(fast.cacheWrite5mUsdPerMillion, published.input * 1.25) ||
        !ratesEqual(fast.cacheWrite1hUsdPerMillion, published.input * 2) ||
        !ratesEqual(fast.cacheReadUsdPerMillion, published.input * 0.1)
      ) {
        finding(
          context,
          "claude",
          "pricing",
          "review-required",
          "price-changed",
          `${model.name}: the bundled Fast price or support differs from the official source.`,
          context.contract.claude.pricingSourceUrl,
        );
      }
    }
  }

  const normalized = plainMarkdown(source);
  const modifierChecks = [
    ratesEqual(manifest.batchFactor, 0.5) && /\b50% discount\b/iu.test(normalized),
    ratesEqual(manifest.usInferenceFactor, 1.1) &&
      /For Claude 4\.6 and later models,[^.]{0,240}\b1\.1x pricing multiplier\b/iu.test(normalized),
    ratesEqual(manifest.webSearchUsdPerThousand, 10) &&
      /\$10 per 1,000 searches/iu.test(normalized),
    /5-minute cache write\s*\|\s*1\.25x base input price/iu.test(source),
    /1-hour cache write\s*\|\s*2x base input price/iu.test(source),
    /Cache read \(hit\)\s*\|\s*0\.1x base input price/iu.test(source),
    /Web fetch usage has no additional charges/iu.test(normalized),
    /Fast mode is not available with the Batch API/iu.test(normalized),
  ];
  if (modifierChecks.some((matches) => !matches)) {
    finding(
      context,
      "claude",
      "pricing",
      "review-required",
      "pricing-modifier-changed",
      "One or more Anthropic cache, Batch, US inference, web search, or web fetch pricing rules changed.",
      context.contract.claude.pricingSourceUrl,
    );
  }
}

function auditReviewAge(context: AuditContext) {
  try {
    const age = dayNumber(isoDay(context.now)) - dayNumber(context.contract.reviewedAt);
    if (age < 0 || age >= context.contract.reviewEveryDays) {
      for (const provider of ["codex", "claude"] as const) {
        finding(
          context,
          provider,
          "review",
          "review-required",
          "review-overdue",
          `The provider contract review is ${age < 0 ? "dated in the future" : `${age} days old`}; review it every ${context.contract.reviewEveryDays} days.`,
        );
      }
    }
  } catch {
    for (const provider of ["codex", "claude"] as const) {
      finding(
        context,
        provider,
        "review",
        "unavailable",
        "invalid-local-contract",
        "The provider contract review date is invalid.",
      );
    }
  }
}

function reportStatus(findings: ProviderAuditFinding[]): ProviderAuditStatus {
  if (findings.some((entry) => entry.status === "unavailable")) return "unavailable";
  if (findings.some((entry) => entry.status === "review-required")) {
    return "review-required";
  }
  return "pass";
}

function compareFindings(left: ProviderAuditFinding, right: ProviderAuditFinding): number {
  return [left.provider, left.area, left.code, left.summary]
    .join("|")
    .localeCompare([right.provider, right.area, right.code, right.summary].join("|"));
}

function boundFindings(findings: ProviderAuditFinding[]): ProviderAuditFinding[] {
  const sorted = [...findings].sort(compareFindings);
  if (sorted.length <= maxDetailedFindings) return sorted;

  const selected: ProviderAuditFinding[] = [];
  const selectedEntries = new Set<ProviderAuditFinding>();
  const representedReasons = new Set<string>();
  for (const entry of sorted) {
    const reason = `${entry.status}|${entry.provider}|${entry.code}`;
    if (representedReasons.has(reason)) continue;
    representedReasons.add(reason);
    selected.push(entry);
    selectedEntries.add(entry);
    if (selected.length === maxDetailedFindings) break;
  }
  for (const entry of sorted) {
    if (selected.length === maxDetailedFindings) break;
    if (selectedEntries.has(entry)) continue;
    selected.push(entry);
    selectedEntries.add(entry);
  }

  const omitted = sorted.filter((entry) => !selectedEntries.has(entry));
  for (const provider of ["claude", "codex"] as const) {
    const providerOmitted = omitted.filter((entry) => entry.provider === provider);
    if (providerOmitted.length === 0) continue;
    selected.push({
      area: "review",
      code: "findings-truncated",
      provider,
      status: providerOmitted.some((entry) => entry.status === "unavailable")
        ? "unavailable"
        : "review-required",
      summary: `${providerOmitted.length} additional bounded findings were omitted.`,
    });
  }
  return selected.sort(compareFindings);
}

function invalidLocalContractReport(now: Date): ProviderAuditReport {
  const findings: ProviderAuditFinding[] = (["claude", "codex"] as const).map((provider) => ({
    area: "review",
    code: "invalid-local-contract",
    provider,
    status: "unavailable",
    summary: "The local reviewed provider contract is invalid.",
  }));
  return {
    checkedAt: now.toISOString(),
    findings,
    reviewedAt: "unavailable",
    schemaVersion: 1,
    sourceCount: 0,
    status: "unavailable",
  };
}

export async function auditProviderContracts(
  options: ProviderAuditOptions = {},
): Promise<ProviderAuditReport> {
  const root = options.workspaceRoot ?? workspaceRoot;
  const now = options.now ?? new Date();
  const readText = options.readText ?? ((path: string) => readFileSync(path, "utf8"));
  let contract: ReviewedProviderContract;
  try {
    contract = options.contract
      ? parseReviewedContract(JSON.stringify(options.contract))
      : parseReviewedContract(
          readText(
            resolve(root, "apps", "desktop", "src-tauri", "provider-contracts", "reviewed.json"),
          ),
        );
  } catch {
    return invalidLocalContractReport(now);
  }
  const context: AuditContext = {
    contract,
    fetcher: options.fetcher ?? globalThis.fetch.bind(globalThis),
    findings: [],
    now,
    readText,
    sources: new Set(),
    workspaceRoot: root,
  };
  auditReviewAge(context);
  await Promise.all([auditCodex(context), auditClaude(context)]);
  const findings = boundFindings(context.findings);
  return {
    checkedAt: context.now.toISOString(),
    findings,
    reviewedAt: contract.reviewedAt,
    schemaVersion: 1,
    sourceCount: context.sources.size,
    status: reportStatus(findings),
  };
}

export function renderProviderAuditMarkdown(report: ProviderAuditReport): string {
  const reportText = (value: string, maxLength: number) => {
    const normalized = value
      .replace(/[\u0000-\u001f\u007f`<>\[\]]/gu, " ")
      .replaceAll("@", "@\u200b")
      .replace(/\s+/gu, " ")
      .trim();
    return normalized.length <= maxLength ? normalized : `${normalized.slice(0, maxLength - 3)}...`;
  };
  const checkedAt = reportText(report.checkedAt, 64);
  const reviewedAt = reportText(report.reviewedAt, 64);
  const lines = [
    "# Provider contract audit",
    "",
    `**Status:** \`${report.status}\``,
    "",
    `Checked: \`${checkedAt}\`  `,
    `Reviewed checkpoint: \`${reviewedAt}\`  `,
    `Authoritative public sources checked: \`${report.sourceCount}\``,
    "",
  ];
  if (report.findings.length === 0) {
    lines.push(
      "The authoritative public usage and pricing evidence matches the reviewed contract.",
    );
  } else {
    lines.push("## Findings", "");
    const renderedFindings = report.findings.slice(0, maxDetailedFindings + 2);
    for (const entry of renderedFindings) {
      let safeSourceUrl: string | undefined;
      if (entry.sourceUrl) {
        try {
          safeSourceUrl = sourceUrl(entry.sourceUrl).toString();
        } catch {
          safeSourceUrl = undefined;
        }
      }
      const source = safeSourceUrl ? ` ([source](${safeSourceUrl}))` : "";
      const code = safeIdentifier(entry.code, 64) ?? "invalid-code";
      const summary = reportText(entry.summary, maxFindingSummaryChars);
      lines.push(
        `- \`${entry.status}\` \`${entry.provider}/${entry.area}/${code}\`: ${summary}${source}`,
      );
    }
    if (report.findings.length > renderedFindings.length) {
      lines.push(
        `- \`review-required\` \`codex/review/findings-truncated\`: ${report.findings.length - renderedFindings.length} additional report findings were omitted.`,
      );
    }
  }
  lines.push(
    "",
    "This read-only audit did not change a parser, fixture, price, or dependency. Its output contains no provider credentials, account data, local paths, transcripts, prompts, or raw remote source bodies.",
  );
  return lines.join("\n");
}

function parseArguments(argumentsList: string[]): "json" | "markdown" {
  if (argumentsList.length === 0) return "markdown";
  if (
    argumentsList.length === 2 &&
    argumentsList[0] === "--format" &&
    ["json", "markdown"].includes(argumentsList[1] ?? "")
  ) {
    return argumentsList[1] as "json" | "markdown";
  }
  throw new Error("Use: bun run audit:providers -- --format <markdown|json>");
}

export async function main(argumentsList = process.argv.slice(2)) {
  let format: "json" | "markdown";
  try {
    format = parseArguments(argumentsList);
  } catch (error) {
    console.error(error instanceof Error ? error.message : "The arguments are invalid.");
    process.exitCode = 64;
    return;
  }
  try {
    const report = await auditProviderContracts();
    console.log(
      format === "json" ? JSON.stringify(report, null, 2) : renderProviderAuditMarkdown(report),
    );
    process.exitCode = report.status === "pass" ? 0 : report.status === "review-required" ? 2 : 3;
  } catch {
    const unavailable = invalidLocalContractReport(new Date());
    console.log(
      format === "json"
        ? JSON.stringify(unavailable, null, 2)
        : renderProviderAuditMarkdown(unavailable),
    );
    process.exitCode = 3;
  }
}

if (import.meta.main) await main();
