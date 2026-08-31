import {
  ADD_TOKENMAXXER_CONTRACT_VERSION,
  addTokenmaxxerOutcomeSchema,
  doomerboardViewSchema,
  type AddTokenmaxxerOutcome,
  type DoomerboardView,
} from "@touchgrass/contracts";
import { queryOptions, type QueryClient } from "@tanstack/react-query";

const doomerboardStaleTimeMs = 5 * 60 * 1_000;
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
  query: DoomerboardQuery;
  reject: (reason?: unknown) => void;
  resolve: (outcome: DoomerboardReadOutcome) => void;
};
type ScheduleDoomerboardRead = (query: DoomerboardQuery) => Promise<DoomerboardReadOutcome>;

type CreateDoomerboardQueryOptionsInput = {
  native: DoomerboardQueryPort;
  profileKey: string;
  rankingDay?: string | undefined;
  selection: DoomerboardQuery;
};

type PrefetchDoomerboardSelectionsInput = Omit<CreateDoomerboardQueryOptionsInput, "selection"> & {
  activeSelection: DoomerboardQuery;
  client: QueryClient;
};

const doomerboardReadSchedulers = new WeakMap<DoomerboardQueryPort, ScheduleDoomerboardRead>();

function createDoomerboardReadScheduler(native: DoomerboardQueryPort): ScheduleDoomerboardRead {
  const pending: PendingDoomerboardRead[] = [];
  let activeReads = 0;
  const startPendingReads = () => {
    while (activeReads < doomerboardNativeReadLimit) {
      const read = pending.shift();
      if (read === undefined) return;
      activeReads += 1;
      void Promise.resolve()
        .then(() => native.read(read.query))
        .then(read.resolve, read.reject)
        .finally(() => {
          activeReads -= 1;
          startPendingReads();
        });
    }
  };
  return (query) =>
    new Promise((resolve, reject) => {
      pending.push({ query, reject, resolve });
      startPendingReads();
    });
}

function scheduleDoomerboardRead(native: DoomerboardQueryPort, query: DoomerboardQuery) {
  let schedule = doomerboardReadSchedulers.get(native);
  if (schedule === undefined) {
    schedule = createDoomerboardReadScheduler(native);
    doomerboardReadSchedulers.set(native, schedule);
  }
  return schedule(query);
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

function doomerboardAudienceKey(profileKey: string, rankingDay: string, audience: string) {
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
    gcTime: 30 * 60 * 1_000,
    queryFn: async (): Promise<DoomerboardView> => {
      const outcome = await scheduleDoomerboardRead(native, selection);
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
}: PrefetchDoomerboardSelectionsInput) {
  const pending = allDoomerboardSelections.filter(
    (selection) => !sameDoomerboardQuery(selection, activeSelection),
  );
  let nextIndex = 0;
  const prefetchNext = async (): Promise<void> => {
    const selection = pending[nextIndex];
    nextIndex += 1;
    if (selection === undefined) return;
    await client.prefetchQuery(
      createDoomerboardQueryOptions({ native, profileKey, rankingDay, selection }),
    );
    await prefetchNext();
  };
  await Promise.all(Array.from({ length: Math.min(3, pending.length) }, prefetchNext));
}

export {
  addTokenmaxxer,
  allDoomerboardSelections,
  createDoomerboardQueryOptions,
  currentRankingDay,
  defaultDoomerboardQuery,
  doomerboardAudienceKey,
  doomerboardRankingDayKey,
  prefetchDoomerboardSelections,
};
export type { DoomerboardPort, DoomerboardPortOutcome, DoomerboardQuery, DoomerboardQueryPort };
