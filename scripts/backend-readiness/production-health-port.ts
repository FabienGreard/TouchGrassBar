import { internal } from "../../packages/backend/convex/_generated/api";
import { adminClient } from "./admin-client";
import type { ProductionConfiguration } from "./production-configuration";
import { readBoundedResponseText } from "./response-body";

const MAX_LOG_RESPONSE_BYTES = 8 * 1_024 * 1_024;
const MAX_LOG_PAGES = 32;

type FunctionLog = {
  error?: string | null;
  identifier: string;
  kind: "Completion" | "Progress";
  timestamp: number;
};

function functionLogPage(value: unknown): { logs: FunctionLog[]; newCursor: number } {
  if (
    typeof value !== "object" ||
    value === null ||
    !("entries" in value) ||
    !("newCursor" in value)
  ) {
    throw new Error("Production health log response is invalid");
  }
  const page = value as { entries?: unknown; newCursor?: unknown };
  const entries = page.entries;
  if (!Number.isSafeInteger(page.newCursor) || Number(page.newCursor) < 0) {
    throw new Error("Production health log response is invalid");
  }
  if (!Array.isArray(entries)) throw new Error("Production health log response is invalid");
  const logs = entries.map((entry) => {
    if (typeof entry !== "object" || entry === null) {
      throw new Error("Production health log response is invalid");
    }
    const candidate = entry as Record<string, unknown>;
    const { error, identifier, kind, timestamp } = candidate;
    if (
      (kind !== "Completion" && kind !== "Progress") ||
      typeof identifier !== "string" ||
      typeof timestamp !== "number"
    ) {
      throw new Error("Production health log response is invalid");
    }
    if (error !== undefined && error !== null && typeof error !== "string") {
      throw new Error("Production health log response is invalid");
    }
    const log: FunctionLog = { identifier, kind, timestamp };
    if (typeof error === "string" || error === null) log.error = error;
    return log;
  });
  return { logs, newCursor: page.newCursor as number };
}

type FetchLogPage = (
  cursor: number,
  remainingBytes: number,
) => Promise<{ byteLength: number; value: unknown }>;

export async function collectFunctionLogs(
  fetchPage: FetchLogPage,
  startedAtMs: number,
  completedAtMs: number,
) {
  let cursor = Math.max(0, startedAtMs - 1_000);
  let totalBytes = 0;
  const logs: FunctionLog[] = [];
  for (let pageNumber = 0; pageNumber < MAX_LOG_PAGES && cursor < completedAtMs; pageNumber += 1) {
    const response = await fetchPage(cursor, MAX_LOG_RESPONSE_BYTES - totalBytes);
    totalBytes += response.byteLength;
    if (totalBytes > MAX_LOG_RESPONSE_BYTES) {
      throw new Error("Production health log read exceeded its bounded policy");
    }
    const page = functionLogPage(response.value);
    if (page.newCursor <= cursor) {
      throw new Error("Production health log cursor did not advance");
    }
    logs.push(...page.logs);
    cursor = page.newCursor;
  }
  if (cursor < completedAtMs) {
    throw new Error("Production health log window is incomplete");
  }
  return logs;
}

export async function productionHealthInputs(
  configuration: ProductionConfiguration,
  startedAtMs: number,
) {
  const client = adminClient(configuration.deployment.url, configuration.adminKey);
  const inspection = await client.action(internal.internal.readiness.inspectHealth, {});
  const completedAtMs = Date.now();
  const logs = await collectFunctionLogs(
    async (cursor, remainingBytes) => {
      const response = await fetch(
        `${configuration.deployment.url}/api/stream_function_logs?cursor=${cursor}`,
        {
          headers: {
            authorization: `Convex ${configuration.adminKey}`,
            "content-type": "application/json",
            "convex-client": "touchgrass-backend-readiness-1",
          },
        },
      );
      if (!response.ok) {
        await response.body?.cancel();
        throw new Error("Production health log read failed");
      }
      const { byteLength, text: body } = await readBoundedResponseText(
        response,
        remainingBytes,
        "Production health log read exceeded its bounded policy",
      );
      let parsed: unknown;
      try {
        parsed = JSON.parse(body) as unknown;
      } catch {
        throw new Error("Production health log response is invalid");
      }
      return { byteLength, value: parsed };
    },
    startedAtMs,
    completedAtMs,
  );
  return { completedAtMs, inspection, logs };
}
