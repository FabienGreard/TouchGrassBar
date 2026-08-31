// @vitest-environment happy-dom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { PanelScreen } from "@/components/panel/panel-screen";
import { createBrowserSanitizedDesktopStateAdapter } from "@/dev/browser-sanitized-desktop-state-adapter";
import type { DoomerboardPort } from "@/native-state/doomerboard-query";
import { createSanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));

afterEach(cleanup);

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
