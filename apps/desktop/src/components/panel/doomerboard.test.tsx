// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { Doomerboard } from "./doomerboard";

afterEach(cleanup);

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
