// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { Doomerboard } from "./doomerboard";

afterEach(cleanup);

test("friend actions cover podium and ledger rows and exclude the current Profile and Global", () => {
  const currentProfile = { displayName: "You", touchGrassId: "#TG-7K4P9D" };
  const rows = [
    currentProfile,
    ...["TG-234567", "TG-234568", "TG-234569"].map((touchGrassId, index) => ({
      displayName: `Friend ${index + 1}`,
      touchGrassId,
    })),
  ].map((profile, index) => Object.assign({}, profile, { rank: index + 1, tokenScore: "100" }));
  const onRemoveFriend = vi.fn(async () => true);
  const view = render(
    <Doomerboard
      currentProfile={currentProfile}
      onRemoveFriend={onRemoveFriend}
      selection={{ audience: "mine", scope: "combined", windowDays: 1 }}
      tokenmaxxerRows={rows}
    />,
  );
  expect(screen.getAllByRole("button", { name: /Friend actions for/ })).toHaveLength(3);
  expect(screen.queryByRole("button", { name: "Friend actions for You" })).toBeNull();
  expect(
    screen
      .getByRole("button", { name: "Friend actions for Friend 3" })
      .closest('[data-slot="doomerboard-ledger"]'),
  ).toBeTruthy();
  view.rerender(
    <Doomerboard currentProfile={currentProfile} onRemoveFriend={onRemoveFriend} rows={rows} />,
  );
  expect(screen.queryByRole("button", { name: /Friend actions for/ })).toBeNull();
});

test("friend removal blocks repeated activation and shows a retry after failure", async () => {
  let finish!: (removed: boolean) => void;
  const onRemoveFriend = vi.fn(
    () =>
      new Promise<boolean>((resolve) => {
        finish = resolve;
      }),
  );
  render(
    <Doomerboard
      currentProfile={{ displayName: "You", touchGrassId: "#TG-7K4P9D" }}
      onRemoveFriend={onRemoveFriend}
      selection={{ audience: "mine", scope: "combined", windowDays: 1 }}
      tokenmaxxerRows={[
        { displayName: "Friend", rank: 1, tokenScore: "100", touchGrassId: "#TG-234567" },
      ]}
    />,
  );
  fireEvent.keyDown(screen.getByRole("button", { name: "Friend actions for Friend" }), {
    key: "Enter",
  });
  fireEvent.click(await screen.findByRole("menuitem", { name: "Remove friend" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Removing…" }));
  expect(onRemoveFriend).toHaveBeenCalledExactlyOnceWith("TG-234567");
  finish(false);
  expect((await screen.findByRole("alert")).textContent).toBe(
    "Could not remove friend. Try again.",
  );
  fireEvent.click(screen.getByRole("menuitem", { name: "Remove friend" }));
  expect(onRemoveFriend).toHaveBeenCalledTimes(2);
  finish(true);
  await waitFor(() => expect(screen.queryByRole("menuitem")).toBeNull());
});

test("copying the current Profile writes only its canonical TouchGrass ID", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  render(
    <Doomerboard
      currentProfile={{ displayName: "Fabien", touchGrassId: "#TG-7K4P9D" }}
      rows={[]}
    />,
  );

  fireEvent.click(screen.getByRole("button", { name: "Copy TouchGrass ID TG-7K4P9D" }));

  await waitFor(() => expect(writeText).toHaveBeenCalledWith("TG-7K4P9D"));
});

test("shows an estimated API-equivalent cost when it is available", () => {
  render(
    <Doomerboard
      rows={[
        {
          apiEquivalentCost: "≈ $12.50",
          displayName: "Fabien",
          rank: 1,
          tokenScore: "4.2M",
          touchGrassId: "#TG-7K4P9D",
        },
      ]}
    />,
  );

  expect(screen.getByLabelText("Estimated API-equivalent cost ≈ $12.50")).toBeTruthy();
});

test("loading keeps the real gold, silver, and bronze podium tones", () => {
  render(<Doomerboard loading />);

  const loading = screen.getByRole("status", { name: "Loading Doomerboard" });
  const podium = [
    { border: "border-rank-silver-border", color: "bg-rank-silver", rank: "2" },
    { border: "border-rank-gold-border", color: "bg-rank-gold", rank: "1" },
    { border: "border-rank-bronze-border", color: "bg-rank-bronze", rank: "3" },
  ];

  for (const expected of podium) {
    const card = loading.querySelector<HTMLElement>(
      `[data-doomerboard-skeleton-rank="${expected.rank}"]`,
    );
    expect(card?.classList.contains(expected.border)).toBe(true);
    expect(card?.classList.contains(expected.color)).toBe(true);
    expect(card?.querySelector("[data-slot='doomerboard-skeleton-medal']")?.classList).toContain(
      expected.color,
    );
  }
});

test("audience hover and focus announce the exact Doomerboard selection", () => {
  const onSelectionIntent = vi.fn();
  render(<Doomerboard onSelectionIntent={onSelectionIntent} rows={[]} />);

  const myTokenmaxxers = screen.getByRole("tab", { name: "Friends" });
  fireEvent.pointerEnter(myTokenmaxxers);
  fireEvent.focus(myTokenmaxxers);

  expect(onSelectionIntent).toHaveBeenNthCalledWith(1, {
    audience: "mine",
    scope: "combined",
    windowDays: 1,
  });
  expect(onSelectionIntent).toHaveBeenNthCalledWith(2, {
    audience: "mine",
    scope: "combined",
    windowDays: 1,
  });
});

test("period and Coding Provider intent announce their alternative selections", () => {
  const onSelectionIntent = vi.fn();
  render(
    <Doomerboard
      onSelectionIntent={onSelectionIntent}
      providers={[
        { displayName: "Codex", provider: "codex" },
        { displayName: "Claude", provider: "claude" },
      ]}
      rows={[]}
    />,
  );

  fireEvent.pointerEnter(screen.getByRole("button", { name: "Select Leaderboard period" }));
  expect(onSelectionIntent).toHaveBeenNthCalledWith(1, {
    audience: "global",
    scope: "combined",
    windowDays: 7,
  });
  expect(onSelectionIntent).toHaveBeenNthCalledWith(2, {
    audience: "global",
    scope: "combined",
    windowDays: 30,
  });

  onSelectionIntent.mockClear();
  fireEvent.focus(screen.getByRole("button", { name: "Select Leaderboard provider" }));
  expect(onSelectionIntent).toHaveBeenNthCalledWith(1, {
    audience: "global",
    scope: "codex",
    windowDays: 1,
  });
  expect(onSelectionIntent).toHaveBeenNthCalledWith(2, {
    audience: "global",
    scope: "claude",
    windowDays: 1,
  });
});
