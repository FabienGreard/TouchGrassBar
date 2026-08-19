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
  namespace: string;
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
  const publicScores = new Map<GenericId<"publicUsages">, PublicScore>();
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
      if (publicScores.has(row.id)) {
        throw new Error("Public Score uniqueness invariant failed");
      }
      if (!knownNamespaces.has(row.boardKey)) invalidEntries += 1;
      publicScores.set(row.id, {
        key: doomerboardKey(row.tokenScore, row.touchGrassId),
        namespace: row.boardKey,
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

  const matched = new Set<GenericId<"publicUsages">>();
  const repairingExpectedKeys = new Set<GenericId<"publicUsages">>();
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
        const expected = publicScores.get(row.id);
        const namespaceIsValid = knownNamespaces.has(namespace);
        const namespaceMatches = expected?.namespace === namespace;
        if (!namespaceIsValid) invalidEntries += 1;
        if (
          !expected ||
          !namespaceIsValid ||
          !namespaceMatches ||
          matched.has(row.id)
        ) {
          extraEntries += 1;
          repairs.push({
            id: row.id,
            namespace,
            observedKey: row.key,
          });
        } else if (!matchesKey(row.key, expected.key)) {
          mismatchedEntries += 1;
          repairingExpectedKeys.add(row.id);
          repairs.push({
            id: row.id,
            namespace,
            observedKey: row.key,
          });
        } else {
          matched.add(row.id);
        }
      }
      complete = page.isDone;
      cursor = page.cursor;
    }
    if (!complete) {
      throw new Error("Doomerboard invariant exceeded its bounded policy");
    }
  }

  for (const [id, expected] of publicScores) {
    if (matched.has(id) || repairingExpectedKeys.has(id)) continue;
    repairs.push({ id, namespace: expected.namespace, observedKey: null });
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
