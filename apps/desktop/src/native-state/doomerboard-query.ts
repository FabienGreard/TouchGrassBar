import {
  ADD_TOKENMAXXER_CONTRACT_VERSION,
  addTokenmaxxerOutcomeSchema,
  doomerboardViewSchema,
  type AddTokenmaxxerOutcome,
  type DoomerboardView,
} from "@touchgrass/contracts";
import { queryOptions, type QueryClient } from "@tanstack/react-query";

const doomerboardStaleTimeMs = 5 * 60 * 1_000;
const doomerboardRankingDayCacheTimeMs = 24 * 60 * 60 * 1_000;
const doomerboardNativeReadLimit = 3;
const defaultDoomerboardQuery: DoomerboardQuery = {
  audience: "global",
  scope: "combined",
  windowDays: 1,
};
const doomerboardAudiences = ["global", "mine"] as const;
const doomerboardScopes = ["combined", "codex", "claude"] as const;
const doomerboardWindows = [1, 7, 30] as const;
const allDoomerboardSelections: readonly DoomerboardQuery[] = doomerboardAudiences.flatMap(
  (audience) =>
    doomerboardScopes.flatMap((scope) =>
      doomerboardWindows.map((windowDays) => ({ audience, scope, windowDays })),
    ),
);

type DoomerboardPortOutcome<Value> =
  | { ok: true; value: Value }
  | { fault: { code: "doomerboard-unavailable" }; ok: false };

type DoomerboardQuery = {
  audience: "global" | "mine";
  scope: "claude" | "codex" | "combined";
  windowDays: 1 | 7 | 30;
};

type DoomerboardPort = {
  add: (profileKey: string, touchGrassId: string) => Promise<DoomerboardPortOutcome<unknown>>;
  read: (
    query: DoomerboardQuery,
    signal?: AbortSignal | undefined,
  ) => Promise<DoomerboardPortOutcome<unknown>>;
  subscribe: (receive: () => void) => Promise<DoomerboardPortOutcome<() => void>>;
  subscribeFocus: (
    receive: (focused: boolean) => void,
  ) => Promise<DoomerboardPortOutcome<() => void>>;
};

type DoomerboardQueryPort = Pick<DoomerboardPort, "read">;
type DoomerboardMutationPort = Pick<DoomerboardPort, "add">;
type DoomerboardReadOutcome = Awaited<ReturnType<DoomerboardQueryPort["read"]>>;
type PendingDoomerboardRead = {
  controller: AbortController;
  profileKey: string;
  query: DoomerboardQuery;
  rankingDay: string;
  reject: (reason?: unknown) => void;
  resolve: (outcome: DoomerboardReadOutcome) => void;
};
type DoomerboardReadTarget = {
  audience?: DoomerboardQuery["audience"] | undefined;
  profileKey: string;
  rankingDay?: string | undefined;
};
type DoomerboardReadScheduler = {
  cancel: (target: DoomerboardReadTarget) => void;
  schedule: (
    query: DoomerboardQuery,
    profileKey: string,
    rankingDay: string,
  ) => Promise<DoomerboardReadOutcome>;
};

type CreateDoomerboardQueryOptionsInput = {
  native: DoomerboardQueryPort;
  profileKey: string;
  rankingDay?: string | undefined;
  selection: DoomerboardQuery;
};

type PrefetchDoomerboardSelectionsInput = Omit<CreateDoomerboardQueryOptionsInput, "selection"> & {
  activeSelection: DoomerboardQuery;
  client: QueryClient;
  signal?: AbortSignal | undefined;
};

const doomerboardReadSchedulers = new WeakMap<DoomerboardQueryPort, DoomerboardReadScheduler>();

function canceledDoomerboardRead() {
  return new DOMException("Doomerboard read canceled", "AbortError");
}

function matchesDoomerboardRead(read: PendingDoomerboardRead, target: DoomerboardReadTarget) {
  return (
    read.profileKey === target.profileKey &&
    (target.rankingDay === undefined || read.rankingDay === target.rankingDay) &&
    (target.audience === undefined || read.query.audience === target.audience)
  );
}

function createDoomerboardReadScheduler(native: DoomerboardQueryPort): DoomerboardReadScheduler {
  const active = new Set<PendingDoomerboardRead>();
  const pending: PendingDoomerboardRead[] = [];
  const startPendingReads = () => {
    while (active.size < doomerboardNativeReadLimit) {
      const read = pending.shift();
      if (read === undefined) return;
      active.add(read);
      void Promise.resolve()
        .then(() => native.read(read.query, read.controller.signal))
        .then(read.resolve, read.reject)
        .finally(() => {
          active.delete(read);
          startPendingReads();
        });
    }
  };
  return {
    cancel: (target) => {
      for (let index = pending.length - 1; index >= 0; index -= 1) {
        const read = pending[index];
        if (read === undefined || !matchesDoomerboardRead(read, target)) continue;
        pending.splice(index, 1);
        read.reject(canceledDoomerboardRead());
      }
      for (const read of active) {
        if (matchesDoomerboardRead(read, target)) {
          read.controller.abort();
          read.reject(canceledDoomerboardRead());
        }
      }
      startPendingReads();
    },
    schedule: (query, profileKey, rankingDay) =>
      new Promise((resolve, reject) => {
        pending.push({
          controller: new AbortController(),
          profileKey,
          query,
          rankingDay,
          reject,
          resolve,
        });
        startPendingReads();
      }),
  };
}

function scheduleDoomerboardRead(
  native: DoomerboardQueryPort,
  query: DoomerboardQuery,
  profileKey: string,
  rankingDay: string,
) {
  let scheduler = doomerboardReadSchedulers.get(native);
  if (scheduler === undefined) {
    scheduler = createDoomerboardReadScheduler(native);
    doomerboardReadSchedulers.set(native, scheduler);
  }
  return scheduler.schedule(query, profileKey, rankingDay);
}

function currentRankingDay(now = new Date()) {
  return now.toISOString().slice(0, 10);
}

function doomerboardQueryKey(profileKey: string, rankingDay: string, selection: DoomerboardQuery) {
  return [
    "doomerboard",
    profileKey,
    rankingDay,
    selection.audience,
    selection.scope,
    selection.windowDays,
  ] as const;
}

function doomerboardRankingDayKey(profileKey: string, rankingDay: string) {
  return ["doomerboard", profileKey, rankingDay] as const;
}

function cancelDoomerboardRankingDay(
  client: QueryClient,
  native: DoomerboardQueryPort,
  profileKey: string,
  rankingDay: string,
) {
  const cancellation = client.cancelQueries({
    queryKey: doomerboardRankingDayKey(profileKey, rankingDay),
  });
  doomerboardReadSchedulers.get(native)?.cancel({ profileKey, rankingDay });
  return cancellation;
}

function cancelDoomerboardAudience(
  client: QueryClient,
  native: DoomerboardQueryPort,
  profileKey: string,
  audience: DoomerboardQuery["audience"],
) {
  const filters = doomerboardProfileAudienceFilter(profileKey, audience);
  const cancellation = client.cancelQueries({
    ...filters,
  });
  doomerboardReadSchedulers.get(native)?.cancel({ audience, profileKey });
  return cancellation;
}

function doomerboardProfileAudienceFilter(
  profileKey: string,
  audience: DoomerboardQuery["audience"],
) {
  return {
    predicate: (query: { queryKey: readonly unknown[] }) => query.queryKey[3] === audience,
    queryKey: ["doomerboard", profileKey] as const,
  };
}

function doomerboardAudienceKey(
  profileKey: string,
  rankingDay: string,
  audience: DoomerboardQuery["audience"],
) {
  return ["doomerboard", profileKey, rankingDay, audience] as const;
}

function sameDoomerboardQuery(left: DoomerboardQuery, right: DoomerboardQuery) {
  return (
    left.audience === right.audience &&
    left.scope === right.scope &&
    left.windowDays === right.windowDays
  );
}

function prioritizedDoomerboardSelections(activeSelection: DoomerboardQuery) {
  const nearestSelections: DoomerboardQuery[] = [
    {
      ...activeSelection,
      audience: activeSelection.audience === "global" ? "mine" : "global",
    },
    ...doomerboardWindows
      .filter((windowDays) => windowDays !== activeSelection.windowDays)
      .map((windowDays) => ({
        audience: activeSelection.audience,
        scope: activeSelection.scope,
        windowDays,
      })),
    ...doomerboardScopes
      .filter((scope) => scope !== activeSelection.scope)
      .map((scope) => ({
        audience: activeSelection.audience,
        scope,
        windowDays: activeSelection.windowDays,
      })),
  ];
  return [
    ...nearestSelections,
    ...allDoomerboardSelections.filter(
      (selection) =>
        !sameDoomerboardQuery(selection, activeSelection) &&
        !nearestSelections.some((nearest) => sameDoomerboardQuery(selection, nearest)),
    ),
  ];
}

function createDoomerboardQueryOptions({
  native,
  profileKey,
  rankingDay = currentRankingDay(),
  selection,
}: CreateDoomerboardQueryOptionsInput) {
  return queryOptions({
    gcTime: doomerboardRankingDayCacheTimeMs,
    queryFn: async (): Promise<DoomerboardView> => {
      const outcome = await scheduleDoomerboardRead(native, selection, profileKey, rankingDay);
      if (!outcome.ok) throw new Error("Doomerboard unavailable");
      const parsed = doomerboardViewSchema.safeParse(outcome.value);
      if (!parsed.success || parsed.data.status !== "ready") {
        throw new Error("Doomerboard unavailable");
      }
      return parsed.data;
    },
    queryKey: doomerboardQueryKey(profileKey, rankingDay, selection),
    refetchInterval: doomerboardStaleTimeMs,
    refetchIntervalInBackground: false,
    retry: false,
    staleTime: doomerboardStaleTimeMs,
  });
}

async function addTokenmaxxer(
  native: DoomerboardMutationPort,
  profileKey: string,
  touchGrassId: string,
): Promise<AddTokenmaxxerOutcome> {
  const unavailable = {
    contractVersion: ADD_TOKENMAXXER_CONTRACT_VERSION,
    status: "unavailable" as const,
  };
  try {
    const outcome = await native.add(profileKey, touchGrassId);
    if (!outcome.ok) return unavailable;
    const parsed = addTokenmaxxerOutcomeSchema.safeParse(outcome.value);
    return parsed.success ? parsed.data : unavailable;
  } catch {
    return unavailable;
  }
}

async function prefetchDoomerboardSelections({
  activeSelection,
  client,
  native,
  profileKey,
  rankingDay = currentRankingDay(),
  signal,
}: PrefetchDoomerboardSelectionsInput) {
  if (signal?.aborted) return;
  const cancelPrefetch = () => {
    void cancelDoomerboardRankingDay(client, native, profileKey, rankingDay);
  };
  signal?.addEventListener("abort", cancelPrefetch, { once: true });
  const pending = prioritizedDoomerboardSelections(activeSelection);
  let nextIndex = 0;
  const prefetchNext = async (): Promise<void> => {
    if (signal?.aborted) return;
    const selection = pending[nextIndex];
    nextIndex += 1;
    if (selection === undefined) return;
    await client.prefetchQuery(
      createDoomerboardQueryOptions({ native, profileKey, rankingDay, selection }),
    );
    if (signal?.aborted) return;
    await prefetchNext();
  };
  try {
    await Promise.all(Array.from({ length: Math.min(3, pending.length) }, prefetchNext));
  } finally {
    signal?.removeEventListener("abort", cancelPrefetch);
  }
}

export {
  addTokenmaxxer,
  allDoomerboardSelections,
  cancelDoomerboardAudience,
  cancelDoomerboardRankingDay,
  createDoomerboardQueryOptions,
  currentRankingDay,
  defaultDoomerboardQuery,
  doomerboardAudienceKey,
  doomerboardProfileAudienceFilter,
  doomerboardRankingDayKey,
  prefetchDoomerboardSelections,
};
export type { DoomerboardPort, DoomerboardPortOutcome, DoomerboardQuery, DoomerboardQueryPort };
