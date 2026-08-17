import type { GenericId } from "convex/values";
import type { PaginationResult } from "convex/server";
import { v } from "convex/values";

import { internal } from "../_generated/api";
import { type ActionCtx, internalAction } from "../_generated/server";
import {
  doomerboard,
  doomerboardKey,
  type DoomerboardKey,
  type StoredDoomerboardKey,
} from "../model/doomerboard";
import { SCOPES, WINDOWS, boardKey } from "../model/values";

const PAGE_SIZE = 100;
const MAX_PAGES = 1_000;

type PublicScore = {
  key: DoomerboardKey;
};

type PublicScorePageRow = {
  boardKey: string;
  id: GenericId<"publicUsages">;
  tokenScore: number;
  touchGrassId: string;
};

type RepairRequest = {
  id: GenericId<"publicUsages">;
  namespace: string;
  observedKey: StoredDoomerboardKey | null;
};

type Inspection = {
  counts: {
    aggregateEntries: number;
    extraEntries: number;
    invalidEntries: number;
    mismatchedEntries: number;
    missingEntries: number;
    publicScores: number;
  };
  repairs: RepairRequest[];
};

const invariantResultValidator = v.object({
  aggregateEntries: v.number(),
  extraEntries: v.number(),
  invalidEntries: v.number(),
  mismatchedEntries: v.number(),
  missingEntries: v.number(),
  publicScores: v.number(),
});

function entryKey(namespace: string, id: GenericId<"publicUsages">) {
  return `${namespace}\u0000${id}`;
}

function matchesKey(
  candidate: StoredDoomerboardKey,
  expected: DoomerboardKey,
) {
  return (
    Array.isArray(candidate) &&
    candidate[0] === expected[0] &&
    candidate[1] === expected[1]
  );
}

async function inspectDoomerboard(ctx: ActionCtx): Promise<Inspection> {
  const knownNamespaces = new Set(
    SCOPES.flatMap((scope) =>
      WINDOWS.map((windowDays) => boardKey(scope, windowDays)),
    ),
  );
  const publicScores = new Map<string, PublicScore>();
  let invalidEntries = 0;
  let publicCursor: string | null = null;
  let publicComplete = false;
  for (
    let pageNumber = 0;
    pageNumber < MAX_PAGES && !publicComplete;
    pageNumber += 1
  ) {
    const page: PaginationResult<PublicScorePageRow> = await ctx.runQuery(
      internal.internal.doomerboardInvariantPage.publicScores,
      {
        paginationOpts: {
          cursor: publicCursor,
          maximumRowsRead: PAGE_SIZE,
          numItems: PAGE_SIZE,
        },
      },
    );
    for (const row of page.page) {
      const key = entryKey(row.boardKey, row.id);
      if (publicScores.has(key)) {
        throw new Error("Public Score uniqueness invariant failed");
      }
      if (!knownNamespaces.has(row.boardKey)) invalidEntries += 1;
      publicScores.set(key, {
        key: doomerboardKey(row.tokenScore, row.touchGrassId),
      });
    }
    publicComplete = page.isDone;
    publicCursor = page.continueCursor;
  }
  if (!publicComplete) {
    throw new Error("Doomerboard invariant exceeded its bounded policy");
  }

  const namespaces: string[] = [];
  let namespaceCursor: string | undefined;
  let namespacesComplete = false;
  for (
    let pageNumber = 0;
    pageNumber < MAX_PAGES && !namespacesComplete;
    pageNumber += 1
  ) {
    const page = await doomerboard.paginateNamespaces(
      ctx,
      namespaceCursor,
      PAGE_SIZE,
    );
    namespaces.push(...page.page);
    namespacesComplete = page.isDone;
    namespaceCursor = page.cursor;
  }
  if (!namespacesComplete) {
    throw new Error("Doomerboard namespace scan exceeded its bounded policy");
  }

  const matched = new Set<string>();
  const repairingExpectedKeys = new Set<string>();
  const repairs: RepairRequest[] = [];
  let aggregateEntries = 0;
  let extraEntries = 0;
  let mismatchedEntries = 0;
  for (const namespace of namespaces) {
    let cursor: string | undefined;
    let complete = false;
    for (
      let pageNumber = 0;
      pageNumber < MAX_PAGES && !complete;
      pageNumber += 1
    ) {
      const page = await doomerboard.paginate(ctx, {
        ...(cursor === undefined ? {} : { cursor }),
        namespace,
        order: "asc",
        pageSize: PAGE_SIZE,
      });
      for (const row of page.page) {
        aggregateEntries += 1;
        const key = entryKey(namespace, row.id);
        const expected = publicScores.get(key);
        const namespaceIsValid = knownNamespaces.has(namespace);
        if (!namespaceIsValid) invalidEntries += 1;
        if (!expected || !namespaceIsValid || matched.has(key)) {
          extraEntries += 1;
          repairs.push({
            id: row.id,
            namespace,
            observedKey: row.key,
          });
        } else if (!matchesKey(row.key, expected.key)) {
          mismatchedEntries += 1;
          repairingExpectedKeys.add(key);
          repairs.push({
            id: row.id,
            namespace,
            observedKey: row.key,
          });
        } else {
          matched.add(key);
        }
      }
      complete = page.isDone;
      cursor = page.cursor;
    }
    if (!complete) {
      throw new Error("Doomerboard invariant exceeded its bounded policy");
    }
  }

  for (const key of publicScores.keys()) {
    if (matched.has(key) || repairingExpectedKeys.has(key)) continue;
    const separator = key.indexOf("\u0000");
    const namespace = key.slice(0, separator);
    const id = key.slice(separator + 1) as GenericId<"publicUsages">;
    repairs.push({ id, namespace, observedKey: null });
  }

  return {
    counts: {
      aggregateEntries,
      extraEntries,
      invalidEntries,
      mismatchedEntries,
      missingEntries: publicScores.size - matched.size,
      publicScores: publicScores.size,
    },
    repairs,
  };
}

export const check = internalAction({
  args: {},
  returns: invariantResultValidator,
  handler: async (ctx) => (await inspectDoomerboard(ctx)).counts,
});

export const repair = internalAction({
  args: {},
  returns: v.object({ changedEntries: v.number() }),
  handler: async (ctx) => {
    const inspection = await inspectDoomerboard(ctx);
    for (const request of inspection.repairs) {
      await ctx.runMutation(
        internal.internal.doomerboardInvariantPage.repairEntry,
        request,
      );
    }
    return { changedEntries: inspection.repairs.length };
  },
});
