import { QueryClient } from "@tanstack/react-query";
import { expect, test, vi } from "vitest";

import {
  addTokenmaxxer,
  allDoomerboardSelections,
  cancelDoomerboardAudience,
  cancelDoomerboardRankingDay,
  createDoomerboardQueryOptions,
  defaultDoomerboardQuery,
  prefetchDoomerboardSelections,
  type DoomerboardPortOutcome,
  type DoomerboardQuery,
  type DoomerboardQueryPort,
} from "@/native-state/doomerboard-query";

const readyView = {
  contractVersion: 1,
  rows: [
    {
      apiEquivalentCostUsd: 12.5,
      displayName: "Fabien",
      rank: 1,
      tokenScore: 4_200_000,
      touchGrassId: "TG-234567",
    },
  ],
  status: "ready",
} as const;

function port(): DoomerboardQueryPort {
  return {
    read: vi.fn(async () => ({ ok: true as const, value: readyView })),
  };
}

test("a fresh Doomerboard selection uses one native read", async () => {
  const native = port();
  const client = new QueryClient();
  const options = createDoomerboardQueryOptions({
    native,
    profileKey: "TG-234567",
    selection: defaultDoomerboardQuery,
  });

  await expect(client.fetchQuery(options)).resolves.toEqual(readyView);
  await expect(client.fetchQuery(options)).resolves.toEqual(readyView);

  expect(native.read).toHaveBeenCalledOnce();
});

test("a Doomerboard read rejects fields outside the public contract", async () => {
  const native: DoomerboardQueryPort = {
    read: vi.fn(async () => ({
      ok: true as const,
      value: {
        ...readyView,
        rows: [{ ...readyView.rows[0], providerMessageId: "private" }],
      },
    })),
  };
  const client = new QueryClient();

  await expect(
    client.fetchQuery(
      createDoomerboardQueryOptions({
        native,
        profileKey: "TG-234567",
        selection: defaultDoomerboardQuery,
      }),
    ),
  ).rejects.toThrow("Doomerboard unavailable");
});

test("Add Tokenmaxxer accepts only strict public outcomes", async () => {
  let value: unknown = { contractVersion: 1, status: "added" };
  const add = vi.fn(async () => ({ ok: true as const, value }));

  await expect(addTokenmaxxer({ add }, "TG-234567")).resolves.toEqual({
    contractVersion: 1,
    status: "added",
  });
  value = { contractVersion: 1, session: "private", status: "added" };
  await expect(addTokenmaxxer({ add }, "TG-234567")).resolves.toEqual({
    contractVersion: 1,
    status: "unavailable",
  });
});

test("prefetch makes every Doomerboard selection available from cache", async () => {
  const native = port();
  const client = new QueryClient();
  const profileKey = "TG-234567";
  const rankingDay = "2026-08-31";

  await client.fetchQuery(
    createDoomerboardQueryOptions({
      native,
      profileKey,
      rankingDay,
      selection: defaultDoomerboardQuery,
    }),
  );
  await prefetchDoomerboardSelections({
    activeSelection: defaultDoomerboardQuery,
    client,
    native,
    profileKey,
    rankingDay,
  });

  for (const selection of allDoomerboardSelections) {
    await client.fetchQuery(
      createDoomerboardQueryOptions({ native, profileKey, rankingDay, selection }),
    );
  }

  expect(native.read).toHaveBeenCalledTimes(18);
});

test("prefetched Doomerboard selections stay cached for the Ranking Day", async () => {
  vi.useFakeTimers();
  const native = port();
  const client = new QueryClient();
  const profileKey = "TG-234567";
  const rankingDay = "2026-08-31";

  try {
    await client.fetchQuery(
      createDoomerboardQueryOptions({
        native,
        profileKey,
        rankingDay,
        selection: defaultDoomerboardQuery,
      }),
    );
    await prefetchDoomerboardSelections({
      activeSelection: defaultDoomerboardQuery,
      client,
      native,
      profileKey,
      rankingDay,
    });

    await vi.advanceTimersByTimeAsync(23 * 60 * 60 * 1_000);

    for (const selection of allDoomerboardSelections) {
      const options = createDoomerboardQueryOptions({
        native,
        profileKey,
        rankingDay,
        selection,
      });
      expect(client.getQueryData(options.queryKey)).toEqual(readyView);
    }
  } finally {
    client.clear();
    vi.useRealTimers();
  }
});

test("all Doomerboard queries share one three-read native limit", async () => {
  let activeReads = 0;
  let maximumActiveReads = 0;
  let releaseReads!: () => void;
  const readsReleased = new Promise<void>((resolve) => {
    releaseReads = resolve;
  });
  const native: DoomerboardQueryPort = {
    read: vi.fn(async () => {
      activeReads += 1;
      maximumActiveReads = Math.max(maximumActiveReads, activeReads);
      await readsReleased;
      activeReads -= 1;
      return { ok: true as const, value: readyView };
    }),
  };
  const firstClient = new QueryClient();
  const secondClient = new QueryClient();

  const reads = Promise.all([
    prefetchDoomerboardSelections({
      activeSelection: defaultDoomerboardQuery,
      client: firstClient,
      native,
      profileKey: "TG-234567",
      rankingDay: "2026-08-31",
    }),
    prefetchDoomerboardSelections({
      activeSelection: defaultDoomerboardQuery,
      client: secondClient,
      native,
      profileKey: "TG-765432",
      rankingDay: "2026-09-01",
    }),
    firstClient.fetchQuery(
      createDoomerboardQueryOptions({
        native,
        profileKey: "TG-234567",
        rankingDay: "2026-08-31",
        selection: defaultDoomerboardQuery,
      }),
    ),
  ]);

  await vi.waitFor(() => expect(activeReads).toBeGreaterThanOrEqual(3));
  releaseReads();
  await reads;

  expect(maximumActiveReads).toBe(3);
});

test("canceling queries removes their queued native reads", async () => {
  let releaseReads!: () => void;
  const readsReleased = new Promise<void>((resolve) => {
    releaseReads = resolve;
  });
  const native: DoomerboardQueryPort = {
    read: vi.fn(async () => {
      await readsReleased;
      return { ok: true as const, value: readyView };
    }),
  };
  const runningClient = new QueryClient();
  const canceledClient = new QueryClient();
  const foregroundClient = new QueryClient();
  const runningSelections = allDoomerboardSelections.slice(0, 3);
  const canceledSelections = allDoomerboardSelections.slice(3, 6);
  const foregroundSelection = allDoomerboardSelections[6];
  if (foregroundSelection === undefined) throw new Error("Missing test selection");

  const runningReads = Promise.all(
    runningSelections.map((selection) =>
      runningClient.fetchQuery(
        createDoomerboardQueryOptions({
          native,
          profileKey: "TG-234567",
          rankingDay: "2026-08-31",
          selection,
        }),
      ),
    ),
  );
  const canceledReads = Promise.allSettled(
    canceledSelections.map((selection) =>
      canceledClient.fetchQuery(
        createDoomerboardQueryOptions({
          native,
          profileKey: "TG-765432",
          rankingDay: "2026-08-31",
          selection,
        }),
      ),
    ),
  );

  await vi.waitFor(() => expect(native.read).toHaveBeenCalledTimes(3));
  await cancelDoomerboardRankingDay(canceledClient, native, "TG-765432", "2026-08-31");
  const foregroundRead = foregroundClient.fetchQuery(
    createDoomerboardQueryOptions({
      native,
      profileKey: "TG-999999",
      rankingDay: "2026-08-31",
      selection: foregroundSelection,
    }),
  );

  releaseReads();
  await Promise.all([runningReads, canceledReads, foregroundRead]);

  expect(native.read).toHaveBeenCalledTimes(4);
  expect(vi.mocked(native.read).mock.calls[3]?.[0]).toEqual(foregroundSelection);
  for (const selection of canceledSelections) {
    expect(vi.mocked(native.read).mock.calls.map(([query]) => query)).not.toContain(selection);
  }
});

test("canceling active reads releases native capacity for current work", async () => {
  let activeReads = 0;
  let maximumActiveReads = 0;
  const oldSelections = allDoomerboardSelections.slice(0, 3);
  const currentSelection = allDoomerboardSelections[3];
  if (currentSelection === undefined) throw new Error("Missing test selection");
  const native: DoomerboardQueryPort = {
    read: vi.fn(
      (selection, signal?: AbortSignal) =>
        new Promise<DoomerboardPortOutcome<unknown>>((resolve) => {
          activeReads += 1;
          maximumActiveReads = Math.max(maximumActiveReads, activeReads);
          let completed = false;
          const complete = (value: { ok: true; value: typeof readyView }) => {
            if (completed) return;
            completed = true;
            activeReads -= 1;
            resolve(value);
          };
          if (selection === currentSelection) {
            complete({ ok: true, value: readyView });
            return;
          }
          const cancel = () => complete({ ok: true, value: readyView });
          signal?.addEventListener("abort", cancel, { once: true });
          if (signal?.aborted) cancel();
        }),
    ),
  };
  const oldClient = new QueryClient();
  const currentClient = new QueryClient();
  const oldReads = Promise.allSettled(
    oldSelections.map((selection) =>
      oldClient.fetchQuery(
        createDoomerboardQueryOptions({
          native,
          profileKey: "TG-234567",
          rankingDay: "2026-08-31",
          selection,
        }),
      ),
    ),
  );

  await vi.waitFor(() => expect(activeReads).toBe(3));
  await cancelDoomerboardRankingDay(oldClient, native, "TG-234567", "2026-08-31");
  const currentRead = currentClient.fetchQuery(
    createDoomerboardQueryOptions({
      native,
      profileKey: "TG-765432",
      rankingDay: "2026-08-31",
      selection: currentSelection,
    }),
  );

  await expect(currentRead).resolves.toEqual(readyView);
  await oldReads;
  expect(native.read).toHaveBeenCalledTimes(4);
  expect(maximumActiveReads).toBe(3);
});

test("canceling one audience preserves active reads for the other audience", async () => {
  const releases = new Map<DoomerboardQuery["audience"], () => void>();
  const signals = new Map<DoomerboardQuery["audience"], AbortSignal | undefined>();
  const native: DoomerboardQueryPort = {
    read: vi.fn(
      (selection, signal) =>
        new Promise<DoomerboardPortOutcome<unknown>>((resolve) => {
          signals.set(selection.audience, signal);
          releases.set(selection.audience, () =>
            resolve({ ok: true as const, value: readyView }),
          );
        }),
    ),
  };
  const client = new QueryClient();
  const profileKey = "TG-234567";
  const rankingDay = "2026-08-31";
  const reads = Promise.allSettled([
    client.fetchQuery(
      createDoomerboardQueryOptions({
        native,
        profileKey,
        rankingDay,
        selection: defaultDoomerboardQuery,
      }),
    ),
    client.fetchQuery(
      createDoomerboardQueryOptions({
        native,
        profileKey,
        rankingDay,
        selection: { ...defaultDoomerboardQuery, audience: "mine" },
      }),
    ),
  ]);

  await vi.waitFor(() => expect(native.read).toHaveBeenCalledTimes(2));
  await cancelDoomerboardAudience(client, native, profileKey, rankingDay, "mine");

  expect(signals.get("mine")?.aborted).toBe(true);
  expect(signals.get("global")?.aborted).toBe(false);
  releases.get("mine")?.();
  releases.get("global")?.();
  await reads;
});

test("aborting a prefetch stops its remaining native reads", async () => {
  let releaseReads!: () => void;
  const readsReleased = new Promise<void>((resolve) => {
    releaseReads = resolve;
  });
  const native: DoomerboardQueryPort = {
    read: vi.fn(async () => {
      await readsReleased;
      return { ok: true as const, value: readyView };
    }),
  };
  const controller = new AbortController();
  const prefetch = prefetchDoomerboardSelections({
    activeSelection: defaultDoomerboardQuery,
    client: new QueryClient(),
    native,
    profileKey: "TG-234567",
    rankingDay: "2026-08-31",
    signal: controller.signal,
  });

  await vi.waitFor(() => expect(native.read).toHaveBeenCalledTimes(3));
  controller.abort();
  releaseReads();
  await prefetch;

  expect(native.read).toHaveBeenCalledTimes(3);
});
