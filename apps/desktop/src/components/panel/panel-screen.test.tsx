// @vitest-environment happy-dom

import type { SanitizedDesktopState } from "@touchgrass/contracts";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, expect, test, vi } from "vitest";

import { PanelScreen } from "@/components/panel/panel-screen";
import { createBrowserSanitizedDesktopStateAdapter } from "@/dev/browser-sanitized-desktop-state-adapter";
import type { DoomerboardPort } from "@/native-state/doomerboard-query";
import {
  createSanitizedDesktopStateDelivery,
  type SanitizedDesktopStateDelivery,
} from "@/native-state/sanitized-desktop-state-delivery";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

type TestDoomerboardPort = DoomerboardPort & {
  changed: () => void;
  setScoreVersion: (version: number) => void;
};

function doomerboardPort(): TestDoomerboardPort {
  let receiveChange: (() => void) | undefined;
  let scoreVersion = 1;
  return {
    add: vi.fn(async () => ({
      ok: true as const,
      value: { contractVersion: 1, status: "added" },
    })),
    read: vi.fn(async (selection) => ({
      ok: true as const,
      value: {
        contractVersion: 1,
        rows: [
          {
            displayName: `${selection.audience}-${selection.scope}-${selection.windowDays}-v${scoreVersion}`,
            rank: 1,
            tokenScore: 4_200_000,
            touchGrassId: "TG-7K4P9D",
          },
        ],
        status: "ready",
      },
    })),
    changed: () => receiveChange?.(),
    setScoreVersion: (version) => {
      scoreVersion = version;
    },
    subscribe: vi.fn(async (receive) => {
      receiveChange = receive;
      return { ok: true as const, value: () => undefined };
    }),
    subscribeFocus: vi.fn(async () => ({ ok: true as const, value: () => undefined })),
  };
}

function mutableStateDelivery(initialSnapshot: SanitizedDesktopState) {
  const subscribers = new Set<() => void>();
  let view: ReturnType<SanitizedDesktopStateDelivery["getSnapshot"]> = {
    phase: "ready",
    refreshing: false,
    snapshot: initialSnapshot,
  };
  return {
    delivery: {
      getSnapshot: () => view,
      requestRefresh: async () => undefined,
      subscribe: (notify) => {
        subscribers.add(notify);
        return () => subscribers.delete(notify);
      },
    } satisfies SanitizedDesktopStateDelivery,
    publish: (snapshot: SanitizedDesktopState) => {
      view = { phase: "ready", refreshing: false, snapshot };
      for (const subscriber of subscribers) subscriber();
    },
  };
}

test("switching audience uses the prefetched Doomerboard", async () => {
  const native = doomerboardPort();
  const stateDelivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      "current",
      () => new Date("2026-08-31T10:00:00.000Z"),
      undefined,
      "synced",
    ),
  );
  const client = new QueryClient();

  render(
    <QueryClientProvider client={client}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={stateDelivery} />
    </QueryClientProvider>,
  );

  await screen.findByText("global-combined-1-v1");
  await waitFor(() => expect(native.read).toHaveBeenCalledTimes(18));

  fireEvent.mouseDown(screen.getByRole("tab", { name: "Friends" }), {
    button: 0,
    ctrlKey: false,
  });

  expect(await screen.findByText("mine-combined-1-v1")).toBeTruthy();
  expect(native.read).toHaveBeenCalledTimes(18);
});

test("audience intent prefetches before the current Doomerboard finishes", async () => {
  let announceCurrentRead!: () => void;
  let finishCurrentRead!: () => void;
  const currentReadStarted = new Promise<void>((resolve) => {
    announceCurrentRead = resolve;
  });
  const currentReadFinished = new Promise<void>((resolve) => {
    finishCurrentRead = resolve;
  });
  const native = doomerboardPort();
  const read = native.read;
  native.read = vi.fn(async (selection) => {
    if (
      selection.audience === "global" &&
      selection.scope === "combined" &&
      selection.windowDays === 1
    ) {
      announceCurrentRead();
      await currentReadFinished;
    }
    return read(selection);
  });
  const stateDelivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      "current",
      () => new Date("2026-08-31T10:00:00.000Z"),
      undefined,
      "synced",
    ),
  );

  render(
    <QueryClientProvider client={new QueryClient()}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={stateDelivery} />
    </QueryClientProvider>,
  );

  await currentReadStarted;
  try {
    fireEvent.pointerEnter(screen.getByRole("tab", { name: "Friends" }));
    await waitFor(() =>
      expect(native.read).toHaveBeenCalledWith(
        { audience: "mine", scope: "combined", windowDays: 1 },
        expect.any(AbortSignal),
      ),
    );
    fireEvent.focus(screen.getByRole("tab", { name: "Friends" }));
    const intendedReads = vi
      .mocked(native.read)
      .mock.calls.filter(
        ([selection]) =>
          selection.audience === "mine" &&
          selection.scope === "combined" &&
          selection.windowDays === 1,
      );
    expect(intendedReads).toHaveLength(1);
  } finally {
    finishCurrentRead();
  }
});

test("switching to a pending Doomerboard shows a skeleton until its scores arrive", async () => {
  let announcePendingSelection!: () => void;
  let finishPendingSelection!: () => void;
  const pendingSelectionStarted = new Promise<void>((resolve) => {
    announcePendingSelection = resolve;
  });
  const pendingSelectionFinished = new Promise<void>((resolve) => {
    finishPendingSelection = resolve;
  });
  const native = doomerboardPort();
  const read = native.read;
  let announced = false;
  native.read = vi.fn(async (selection) => {
    if (
      selection.audience === "mine" &&
      selection.scope === "combined" &&
      selection.windowDays === 1
    ) {
      if (!announced) {
        announced = true;
        announcePendingSelection();
      }
      await pendingSelectionFinished;
    }
    return read(selection);
  });
  const stateDelivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      "current",
      () => new Date("2026-08-31T10:00:00.000Z"),
      undefined,
      "synced",
    ),
  );

  render(
    <QueryClientProvider client={new QueryClient()}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={stateDelivery} />
    </QueryClientProvider>,
  );

  await screen.findByText("global-combined-1-v1");
  await pendingSelectionStarted;
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Friends" }), {
    button: 0,
    ctrlKey: false,
  });

  expect(screen.getByRole("status", { name: "Loading Doomerboard" })).toBeTruthy();
  expect(screen.queryByText("Your Leaderboard is lonely")).toBeNull();
  expect(screen.queryByText("Leaderboard unavailable")).toBeNull();

  finishPendingSelection();
  expect(await screen.findByText("mine-combined-1-v1")).toBeTruthy();
});

test("returning to a pending selection reuses its native read", async () => {
  let announcePendingSelection!: () => void;
  let finishPendingSelection!: () => void;
  const pendingSelectionStarted = new Promise<void>((resolve) => {
    announcePendingSelection = resolve;
  });
  const pendingSelectionFinished = new Promise<void>((resolve) => {
    finishPendingSelection = resolve;
  });
  const native = doomerboardPort();
  const read = native.read;
  let announced = false;
  native.read = vi.fn(async (selection) => {
    if (
      selection.audience === "mine" &&
      selection.scope === "combined" &&
      selection.windowDays === 1
    ) {
      if (!announced) {
        announced = true;
        announcePendingSelection();
      }
      await pendingSelectionFinished;
    }
    return read(selection);
  });
  const stateDelivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      "current",
      () => new Date("2026-08-31T10:00:00.000Z"),
      undefined,
      "synced",
    ),
  );

  render(
    <QueryClientProvider client={new QueryClient()}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={stateDelivery} />
    </QueryClientProvider>,
  );

  await screen.findByText("global-combined-1-v1");
  await pendingSelectionStarted;
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Friends" }), {
    button: 0,
    ctrlKey: false,
  });
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Global" }), {
    button: 0,
    ctrlKey: false,
  });
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Friends" }), {
    button: 0,
    ctrlKey: false,
  });

  finishPendingSelection();
  expect(await screen.findByText("mine-combined-1-v1")).toBeTruthy();
  const pendingReads = vi
    .mocked(native.read)
    .mock.calls.filter(
      ([selection]) =>
        selection.audience === "mine" &&
        selection.scope === "combined" &&
        selection.windowDays === 1,
    );
  expect(pendingReads).toHaveLength(1);
});

test("a native revision refreshes only the active Doomerboard", async () => {
  const native = doomerboardPort();
  const stateDelivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      "current",
      () => new Date("2026-08-31T10:00:00.000Z"),
      undefined,
      "synced",
    ),
  );

  render(
    <QueryClientProvider client={new QueryClient()}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={stateDelivery} />
    </QueryClientProvider>,
  );
  await screen.findByText("global-combined-1-v1");
  await waitFor(() => expect(native.read).toHaveBeenCalledTimes(18));

  native.setScoreVersion(2);
  native.changed();

  expect(await screen.findByText("global-combined-1-v2")).toBeTruthy();
  await waitFor(() => expect(native.read).toHaveBeenCalledTimes(19));

  fireEvent.mouseDown(screen.getByRole("tab", { name: "Friends" }), {
    button: 0,
    ctrlKey: false,
  });

  expect(await screen.findByText("mine-combined-1-v2")).toBeTruthy();
  await waitFor(() => expect(native.read).toHaveBeenCalledTimes(20));
});

test("a late prefetch cannot replace a newer Doomerboard revision", async () => {
  let announceOldPrefetch!: () => void;
  let finishOldPrefetch!: () => void;
  const oldPrefetchStarted = new Promise<void>((resolve) => {
    announceOldPrefetch = resolve;
  });
  const oldPrefetchFinished = new Promise<void>((resolve) => {
    finishOldPrefetch = resolve;
  });
  const native = doomerboardPort();
  const read = native.read;
  let oldPrefetchPending = true;
  native.read = vi.fn(async (selection) => {
    const outcome = await read(selection);
    if (
      oldPrefetchPending &&
      selection.audience === "mine" &&
      selection.scope === "combined" &&
      selection.windowDays === 1
    ) {
      oldPrefetchPending = false;
      announceOldPrefetch();
      await oldPrefetchFinished;
    }
    return outcome;
  });
  const stateDelivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      "current",
      () => new Date("2026-08-31T10:00:00.000Z"),
      undefined,
      "synced",
    ),
  );

  render(
    <QueryClientProvider client={new QueryClient()}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={stateDelivery} />
    </QueryClientProvider>,
  );

  await screen.findByText("global-combined-1-v1");
  await oldPrefetchStarted;
  native.setScoreVersion(2);
  native.changed();
  expect(await screen.findByText("global-combined-1-v2")).toBeTruthy();

  finishOldPrefetch();
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Friends" }), {
    button: 0,
    ctrlKey: false,
  });

  expect(await screen.findByText("mine-combined-1-v2")).toBeTruthy();
  expect(screen.queryByText("mine-combined-1-v1")).toBeNull();
});

test("a revision before prefetch starts still warms the new revision", async () => {
  let announceFirstRead!: () => void;
  let finishFirstRead!: () => void;
  const firstReadStarted = new Promise<void>((resolve) => {
    announceFirstRead = resolve;
  });
  const firstReadFinished = new Promise<void>((resolve) => {
    finishFirstRead = resolve;
  });
  const native = doomerboardPort();
  const read = native.read;
  let firstReadPending = true;
  native.read = vi.fn(async (selection) => {
    if (firstReadPending) {
      firstReadPending = false;
      announceFirstRead();
      await firstReadFinished;
    }
    return read(selection);
  });
  const stateDelivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      "current",
      () => new Date("2026-08-31T10:00:00.000Z"),
      undefined,
      "synced",
    ),
  );

  render(
    <QueryClientProvider client={new QueryClient()}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={stateDelivery} />
    </QueryClientProvider>,
  );

  await firstReadStarted;
  native.setScoreVersion(2);
  native.changed();
  finishFirstRead();

  expect(await screen.findByText("global-combined-1-v2")).toBeTruthy();
  await waitFor(() => expect(native.read).toHaveBeenCalledTimes(19));
});

test("StrictMode effect replay still prefetches every selection", async () => {
  let announceFirstRead!: () => void;
  let finishFirstRead!: () => void;
  const firstReadStarted = new Promise<void>((resolve) => {
    announceFirstRead = resolve;
  });
  const firstReadFinished = new Promise<void>((resolve) => {
    finishFirstRead = resolve;
  });
  const native = doomerboardPort();
  const read = native.read;
  let firstReadPending = true;
  native.read = vi.fn(async (selection) => {
    if (firstReadPending) {
      firstReadPending = false;
      announceFirstRead();
      await firstReadFinished;
    }
    return read(selection);
  });
  const stateDelivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      "current",
      () => new Date("2026-08-31T10:00:00.000Z"),
      undefined,
      "synced",
    ),
  );

  render(
    <StrictMode>
      <QueryClientProvider client={new QueryClient()}>
        <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={stateDelivery} />
      </QueryClientProvider>
    </StrictMode>,
  );

  await firstReadStarted;
  finishFirstRead();
  await screen.findByText("global-combined-1-v1");
  await waitFor(() => expect(native.read).toHaveBeenCalledTimes(18));
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Friends" }), {
    button: 0,
    ctrlKey: false,
  });
  expect(await screen.findByText("mine-combined-1-v1")).toBeTruthy();
  expect(native.read).toHaveBeenCalledTimes(18);
});

test("a Profile change discards reads from the previous prefetch", async () => {
  const browser = createBrowserSanitizedDesktopStateAdapter(
    "current",
    () => new Date("2026-08-31T10:00:00.000Z"),
    undefined,
    "synced",
  );
  const initial = await browser.readSnapshot();
  if (!initial.ok) throw new Error("Missing browser snapshot");
  const firstSnapshot = initial.value as SanitizedDesktopState;
  if (firstSnapshot.profile.status !== "ready") throw new Error("Missing ready Profile");
  const secondProfileKey = "TG-765432";
  const secondSnapshot: SanitizedDesktopState = {
    ...firstSnapshot,
    profile: {
      ...firstSnapshot.profile,
      displayName: "Second Profile",
      touchGrassId: secondProfileKey,
    },
    revision: "3",
  };
  const returnedSnapshot: SanitizedDesktopState = { ...firstSnapshot, revision: "4" };
  const state = mutableStateDelivery(firstSnapshot);
  let activeProfile = firstSnapshot.profile.touchGrassId;
  let announceOldPrefetch!: () => void;
  let finishOldPrefetch!: () => void;
  let announceOldPrefetchResult!: () => void;
  const oldPrefetchStarted = new Promise<void>((resolve) => {
    announceOldPrefetch = resolve;
  });
  const oldPrefetchFinished = new Promise<void>((resolve) => {
    finishOldPrefetch = resolve;
  });
  const oldPrefetchResult = new Promise<void>((resolve) => {
    announceOldPrefetchResult = resolve;
  });
  const native = doomerboardPort();
  let oldPrefetchPending = true;
  native.read = vi.fn(async (selection) => {
    const isOldPrefetch =
      oldPrefetchPending &&
      selection.audience === "mine" &&
      selection.scope === "combined" &&
      selection.windowDays === 1;
    if (isOldPrefetch) {
      oldPrefetchPending = false;
      announceOldPrefetch();
      await oldPrefetchFinished;
    }
    const outcome = {
      ok: true as const,
      value: {
        contractVersion: 1,
        rows: [
          {
            displayName: `${activeProfile}-${selection.audience}-${selection.scope}-${selection.windowDays}`,
            rank: 1,
            tokenScore: 4_200_000,
            touchGrassId: activeProfile,
          },
        ],
        status: "ready" as const,
      },
    };
    if (isOldPrefetch) announceOldPrefetchResult();
    return outcome;
  });

  render(
    <QueryClientProvider client={new QueryClient()}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={state.delivery} />
    </QueryClientProvider>,
  );

  await screen.findByText(`${activeProfile}-global-combined-1`);
  await oldPrefetchStarted;
  activeProfile = secondProfileKey;
  state.publish(secondSnapshot);
  await screen.findByText(`${secondProfileKey}-global-combined-1`);

  finishOldPrefetch();
  await oldPrefetchResult;
  activeProfile = firstSnapshot.profile.touchGrassId;
  state.publish(returnedSnapshot);
  await screen.findByText(`${activeProfile}-global-combined-1`);
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Friends" }), {
    button: 0,
    ctrlKey: false,
  });

  expect(await screen.findByText(`${activeProfile}-mine-combined-1`)).toBeTruthy();
  expect(screen.queryByText(`${secondProfileKey}-mine-combined-1`)).toBeNull();
});

test("Add Tokenmaxxer discards a pending Mine result before selecting it", async () => {
  let announceOldMineRead!: () => void;
  let finishOldMineRead!: () => void;
  const oldMineReadStarted = new Promise<void>((resolve) => {
    announceOldMineRead = resolve;
  });
  const oldMineReadFinished = new Promise<void>((resolve) => {
    finishOldMineRead = resolve;
  });
  const native = doomerboardPort();
  let scoreVersion = 1;
  let oldMineReadPending = true;
  native.add = vi.fn(async () => {
    scoreVersion = 2;
    return {
      ok: true as const,
      value: { contractVersion: 1, status: "added" },
    };
  });
  native.read = vi.fn(async (selection) => {
    const oldMineRead =
      oldMineReadPending &&
      selection.audience === "mine" &&
      selection.scope === "combined" &&
      selection.windowDays === 1;
    const readVersion = scoreVersion;
    if (oldMineRead) {
      oldMineReadPending = false;
      announceOldMineRead();
      await oldMineReadFinished;
    }
    return {
      ok: true as const,
      value: {
        contractVersion: 1,
        rows: [
          {
            displayName: `mutation-result-v${readVersion}`,
            rank: 1,
            tokenScore: 4_200_000,
            touchGrassId: "TG-7K4P9D",
          },
        ],
        status: "ready" as const,
      },
    };
  });
  const stateDelivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      "current",
      () => new Date("2026-08-31T10:00:00.000Z"),
      undefined,
      "synced",
    ),
  );

  render(
    <QueryClientProvider client={new QueryClient()}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={stateDelivery} />
    </QueryClientProvider>,
  );

  await screen.findByText("mutation-result-v1");
  await oldMineReadStarted;
  fireEvent.pointerDown(screen.getByRole("button", { name: "Open panel menu" }), {
    button: 0,
    ctrlKey: false,
  });
  fireEvent.click(await screen.findByRole("menuitem", { name: "Add a Tokenmaxxer…" }));
  fireEvent.change(screen.getByLabelText("TouchGrass ID"), {
    target: { value: "TG-234567" },
  });
  const submit = screen.getByRole("button", { name: "Add Tokenmaxxer" }).closest("form");
  if (submit === null) throw new Error("Missing Add Tokenmaxxer form");
  fireEvent.submit(submit);

  await waitFor(() => expect(native.add).toHaveBeenCalledOnce());
  await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  finishOldMineRead();

  expect(await screen.findByText("mutation-result-v2")).toBeTruthy();
  expect(screen.queryByText("mutation-result-v1")).toBeNull();
});

test("Add Tokenmaxxer refreshes Mine when Mine is already selected", async () => {
  const native = doomerboardPort();
  native.add = vi.fn(async () => {
    native.setScoreVersion(2);
    return {
      ok: true as const,
      value: { contractVersion: 1, status: "added" },
    };
  });
  const stateDelivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      "current",
      () => new Date("2026-08-31T10:00:00.000Z"),
      undefined,
      "synced",
    ),
  );

  render(
    <QueryClientProvider client={new QueryClient()}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={stateDelivery} />
    </QueryClientProvider>,
  );

  await screen.findByText("global-combined-1-v1");
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Friends" }), {
    button: 0,
    ctrlKey: false,
  });
  await screen.findByText("mine-combined-1-v1");
  fireEvent.pointerDown(screen.getByRole("button", { name: "Open panel menu" }), {
    button: 0,
    ctrlKey: false,
  });
  fireEvent.click(await screen.findByRole("menuitem", { name: "Add a Tokenmaxxer…" }));
  fireEvent.change(screen.getByLabelText("TouchGrass ID"), {
    target: { value: "TG-234567" },
  });
  const submit = screen.getByRole("button", { name: "Add Tokenmaxxer" }).closest("form");
  if (submit === null) throw new Error("Missing Add Tokenmaxxer form");
  fireEvent.submit(submit);

  expect(await screen.findByText("mine-combined-1-v2")).toBeTruthy();
  expect(screen.queryByText("mine-combined-1-v1")).toBeNull();
});

test("Add Tokenmaxxer refreshes the completion Ranking Day after UTC midnight", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  vi.setSystemTime(new Date("2026-08-31T23:59:59.000Z"));
  const native = doomerboardPort();
  native.add = vi.fn(async () => {
    vi.setSystemTime(new Date("2026-09-01T00:00:01.000Z"));
    native.setScoreVersion(2);
    return {
      ok: true as const,
      value: { contractVersion: 1, status: "added" },
    };
  });
  const stateDelivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      "current",
      () => new Date("2026-08-31T23:59:59.000Z"),
      undefined,
      "synced",
    ),
  );

  render(
    <QueryClientProvider client={new QueryClient()}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={stateDelivery} />
    </QueryClientProvider>,
  );

  await screen.findByText("global-combined-1-v1");
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Friends" }), {
    button: 0,
    ctrlKey: false,
  });
  await screen.findByText("mine-combined-1-v1");
  fireEvent.pointerDown(screen.getByRole("button", { name: "Open panel menu" }), {
    button: 0,
    ctrlKey: false,
  });
  fireEvent.click(await screen.findByRole("menuitem", { name: "Add a Tokenmaxxer…" }));
  fireEvent.change(screen.getByLabelText("TouchGrass ID"), {
    target: { value: "TG-234567" },
  });
  const submit = screen.getByRole("button", { name: "Add Tokenmaxxer" }).closest("form");
  if (submit === null) throw new Error("Missing Add Tokenmaxxer form");
  fireEvent.submit(submit);

  expect(await screen.findByText("mine-combined-1-v2")).toBeTruthy();
  expect(screen.queryByText("mine-combined-1-v1")).toBeNull();
});

test("Add Tokenmaxxer ignores a delayed completion after the Profile changes", async () => {
  const browser = createBrowserSanitizedDesktopStateAdapter(
    "current",
    () => new Date("2026-08-31T10:00:00.000Z"),
    undefined,
    "synced",
  );
  const initial = await browser.readSnapshot();
  if (!initial.ok) throw new Error("Missing browser snapshot");
  const firstSnapshot = initial.value as SanitizedDesktopState;
  if (firstSnapshot.profile.status !== "ready") throw new Error("Missing ready Profile");
  const firstProfileKey = firstSnapshot.profile.touchGrassId;
  const secondProfileKey = "TG-765432";
  const secondSnapshot: SanitizedDesktopState = {
    ...firstSnapshot,
    profile: {
      ...firstSnapshot.profile,
      displayName: "Second Profile",
      touchGrassId: secondProfileKey,
    },
    revision: "3",
  };
  const state = mutableStateDelivery(firstSnapshot);
  let releaseAdd!: () => void;
  const addReleased = new Promise<void>((resolve) => {
    releaseAdd = resolve;
  });
  let announceHandlerTurn!: () => void;
  const handlerHadTurn = new Promise<void>((resolve) => {
    announceHandlerTurn = resolve;
  });
  const native = doomerboardPort();
  native.add = vi.fn(async () => {
    await addReleased;
    setTimeout(announceHandlerTurn, 0);
    return {
      ok: true as const,
      value: { contractVersion: 1, status: "added" },
    };
  });
  const client = new QueryClient();

  render(
    <QueryClientProvider client={client}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={state.delivery} />
    </QueryClientProvider>,
  );

  await screen.findByText("global-combined-1-v1");
  fireEvent.pointerDown(screen.getByRole("button", { name: "Open panel menu" }), {
    button: 0,
    ctrlKey: false,
  });
  fireEvent.click(await screen.findByRole("menuitem", { name: "Add a Tokenmaxxer…" }));
  fireEvent.change(screen.getByLabelText("TouchGrass ID"), {
    target: { value: "TG-234567" },
  });
  const submit = screen.getByRole("button", { name: "Add Tokenmaxxer" }).closest("form");
  if (submit === null) throw new Error("Missing Add Tokenmaxxer form");
  fireEvent.submit(submit);
  await waitFor(() => expect(native.add).toHaveBeenCalledOnce());
  expect(native.add).toHaveBeenCalledWith(firstProfileKey, "TG-234567");

  state.publish(secondSnapshot);
  await screen.findByText("global-combined-1-v1");
  await waitFor(() =>
    expect(client.getQueryCache().findAll({ queryKey: ["doomerboard", firstProfileKey] })).toEqual(
      [],
    ),
  );

  releaseAdd();
  await handlerHadTurn;

  expect(client.getQueryCache().findAll({ queryKey: ["doomerboard", firstProfileKey] })).toEqual(
    [],
  );
  expect(screen.getByRole("tab", { name: "Global" }).getAttribute("data-state")).toBe("active");
});

test("Add Tokenmaxxer ignores a Profile change while Mine cancellation is pending", async () => {
  const browser = createBrowserSanitizedDesktopStateAdapter(
    "current",
    () => new Date("2026-08-31T10:00:00.000Z"),
    undefined,
    "synced",
  );
  const initial = await browser.readSnapshot();
  if (!initial.ok) throw new Error("Missing browser snapshot");
  const firstSnapshot = initial.value as SanitizedDesktopState;
  if (firstSnapshot.profile.status !== "ready") throw new Error("Missing ready Profile");
  const firstProfileKey = firstSnapshot.profile.touchGrassId;
  const secondSnapshot: SanitizedDesktopState = {
    ...firstSnapshot,
    profile: {
      ...firstSnapshot.profile,
      displayName: "Second Profile",
      touchGrassId: "TG-765432",
    },
    revision: "3",
  };
  const state = mutableStateDelivery(firstSnapshot);
  const native = doomerboardPort();
  const client = new QueryClient();

  render(
    <QueryClientProvider client={client}>
      <PanelScreen doomerboardPort={native} hasNativeRuntime stateDelivery={state.delivery} />
    </QueryClientProvider>,
  );

  await screen.findByText("global-combined-1-v1");
  await waitFor(() => expect(native.read).toHaveBeenCalledTimes(18));

  let announceCancellation!: () => void;
  const cancellationStarted = new Promise<void>((resolve) => {
    announceCancellation = resolve;
  });
  let releaseCancellation!: () => void;
  const cancellationReleased = new Promise<void>((resolve) => {
    releaseCancellation = resolve;
  });
  let announceHandlerTurn!: () => void;
  const handlerHadTurn = new Promise<void>((resolve) => {
    announceHandlerTurn = resolve;
  });
  const cancelQueries = client.cancelQueries.bind(client);
  let delayNextCancellation = true;
  vi.spyOn(client, "cancelQueries").mockImplementation(async (...args) => {
    const result = await cancelQueries(...args);
    if (delayNextCancellation) {
      delayNextCancellation = false;
      announceCancellation();
      await cancellationReleased;
      setTimeout(announceHandlerTurn, 0);
    }
    return result;
  });

  fireEvent.pointerDown(screen.getByRole("button", { name: "Open panel menu" }), {
    button: 0,
    ctrlKey: false,
  });
  fireEvent.click(await screen.findByRole("menuitem", { name: "Add a Tokenmaxxer…" }));
  fireEvent.change(screen.getByLabelText("TouchGrass ID"), {
    target: { value: "TG-234567" },
  });
  const submit = screen.getByRole("button", { name: "Add Tokenmaxxer" }).closest("form");
  if (submit === null) throw new Error("Missing Add Tokenmaxxer form");
  fireEvent.submit(submit);
  await cancellationStarted;

  state.publish(secondSnapshot);
  await screen.findByRole("button", { name: "Copy TouchGrass ID TG-765432" });
  releaseCancellation();
  await handlerHadTurn;

  expect(client.getQueryCache().findAll({ queryKey: ["doomerboard", firstProfileKey] })).toEqual(
    [],
  );
  expect(screen.getByRole("tab", { name: "Global" }).getAttribute("data-state")).toBe("active");
});
