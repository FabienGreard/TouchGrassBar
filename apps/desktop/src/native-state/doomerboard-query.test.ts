import { QueryClient } from "@tanstack/react-query";
import { expect, test, vi } from "vitest";

import {
  addTokenmaxxer,
  allDoomerboardSelections,
  createDoomerboardQueryOptions,
  defaultDoomerboardQuery,
  prefetchDoomerboardSelections,
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
