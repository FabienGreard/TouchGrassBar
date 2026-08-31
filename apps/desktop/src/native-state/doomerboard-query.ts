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
  add: (touchGrassId: string) => Promise<DoomerboardPortOutcome<unknown>>;
  read: (query: DoomerboardQuery) => Promise<DoomerboardPortOutcome<unknown>>;
  subscribe: (receive: () => void) => Promise<DoomerboardPortOutcome<() => void>>;
  subscribeFocus: (
    receive: (focused: boolean) => void,
  ) => Promise<DoomerboardPortOutcome<() => void>>;
};

type DoomerboardQueryPort = Pick<DoomerboardPort, "read">;
type DoomerboardMutationPort = Pick<DoomerboardPort, "add">;
type DoomerboardReadOutcome = Awaited<ReturnType<DoomerboardQueryPort["read"]>>;
type PendingDoomerboardRead = {
  profileKey: string;
  query: DoomerboardQuery;
  rankingDay: string;
  reject: (reason?: unknown) => void;
  resolve: (outcome: DoomerboardReadOutcome) => void;
};
type DoomerboardReadScheduler = {
  cancel: (profileKey: string, rankingDay: string) => void;
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

function createDoomerboardReadScheduler(native: DoomerboardQueryPort): DoomerboardReadScheduler {
  const active = new Set<PendingDoomerboardRead>();
  const pending: PendingDoomerboardRead[] = [];
  const startPendingReads = () => {
    while (active.size < doomerboardNativeReadLimit) {
      const read = pending.shift();
      if (read === undefined) return;
      active.add(read);
      void Promise.resolve()
        .then(() => native.read(read.query))
        .then(read.resolve, read.reject)
        .finally(() => {
          active.delete(read);
          startPendingReads();
        });
    }
  };
  return {
    cancel: (profileKey, rankingDay) => {
      for (let index = pending.length - 1; index >= 0; index -= 1) {
        const read = pending[index];
        if (read?.profileKey !== profileKey || read.rankingDay !== rankingDay) continue;
        pending.splice(index, 1);
        read.reject(canceledDoomerboardRead());
      }
      for (const read of active) {
        if (read.profileKey === profileKey && read.rankingDay === rankingDay) {
          read.reject(canceledDoomerboardRead());
        }
      }
      startPendingReads();
    },
    schedule: (query, profileKey, rankingDay) =>
      new Promise((resolve, reject) => {
        pending.push({ profileKey, query, rankingDay, reject, resolve });
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
  doomerboardReadSchedulers.get(native)?.cancel(profileKey, rankingDay);
  return cancellation;
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
  touchGrassId: string,
): Promise<AddTokenmaxxerOutcome> {
  const unavailable = {
    contractVersion: ADD_TOKENMAXXER_CONTRACT_VERSION,
    status: "unavailable" as const,
  };
  try {
    const outcome = await native.add(touchGrassId);
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
  const pending = allDoomerboardSelections.filter(
    (selection) => !sameDoomerboardQuery(selection, activeSelection),
  );
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
  cancelDoomerboardRankingDay,
  createDoomerboardQueryOptions,
  currentRankingDay,
  defaultDoomerboardQuery,
  doomerboardAudienceKey,
  doomerboardRankingDayKey,
  prefetchDoomerboardSelections,
};
export type { DoomerboardPort, DoomerboardPortOutcome, DoomerboardQuery, DoomerboardQueryPort };
