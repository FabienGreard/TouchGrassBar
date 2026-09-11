import { sanitizedDesktopStateSchema } from "@touchgrass/contracts";
// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import type { UsagePeriods } from "@touchgrass/contracts";
import { UsageOverview } from "./usage-overview";
import { createBrowserSanitizedDesktopStateAdapter } from "@/dev/browser-sanitized-desktop-state-adapter";

afterEach(cleanup);
const unavailable: UsagePeriods = {
  scanStatus: "unavailable",
  today: { availability: "unavailable" },
  sevenDays: { availability: "unavailable" },
  thirtyDays: { availability: "unavailable" },
};
async function populated() {
  const result = await createBrowserSanitizedDesktopStateAdapter(
    "current",
    () => new Date("2026-09-11T16:25:00Z"),
  ).readSnapshot();
  if (!result.ok) throw new Error("fixture unavailable");
  const state = sanitizedDesktopStateSchema.parse(result.value);
  return render(
    <UsageOverview
      usage={state.combinedUsage}
      providers={state.providers}
      history={state.usageHistory}
    />,
  );
}
async function select(label: string, value: string) {
  fireEvent.pointerDown(screen.getByRole("button", { name: `Select Usage ${label}` }), {
    button: 0,
    ctrlKey: false,
    pointerType: "mouse",
  });
  fireEvent.click(await screen.findByRole("menuitemradio", { name: value }));
}

test("defaults to Combined and Today with the model below the chart", async () => {
  const view = await populated();
  expect(screen.getByRole("button", { name: "Select Usage period" }).textContent).toBe("Today");
  expect(screen.getByRole("button", { name: "Select Usage provider" }).textContent).toBe(
    "Combined",
  );
  expect(screen.getByLabelText("14,800,000 tokens")).toBeTruthy();
  expect(view.container.querySelectorAll(".usage-column")).toHaveLength(17);
  expect(view.container.querySelector(".usage-footer")?.textContent).toContain("GPT 5.6 Sol");
  expect(screen.queryByRole("heading", { name: "Usage" })).toBeNull();
  expect(view.container.querySelector('[data-slot="metric-gauge"]')).toBeNull();
});

test("provider and period change all values; focused detail has cost, model and provider split", async () => {
  const view = await populated();
  fireEvent.focus(view.container.querySelector(".usage-column:last-of-type")!);
  expect(screen.getByRole("tooltip").textContent).toContain("API equivalent");
  expect(screen.getByRole("tooltip").textContent).not.toContain("≈ $44.86");
  expect(screen.getByRole("tooltip").textContent).toContain("16:00–17:00 UTC");
  expect(screen.getByRole("tooltip").textContent).toContain("GPT 5.6 Sol");
  expect(screen.getByRole("tooltip").textContent).toContain("Claude");
  fireEvent.keyDown(view.container.querySelector(".usage-column:last-of-type")!, { key: "Escape" });
  expect(screen.queryByRole("tooltip")).toBeNull();
  await select("period", "30 days");
  expect(screen.getByLabelText("304,600,000 tokens")).toBeTruthy();
  expect(view.container.querySelectorAll(".usage-column")).toHaveLength(30);
  expect(view.container.querySelector(".usage-footer")?.textContent).toContain("GPT 5.5");
  await select("provider", "Claude");
  expect(screen.getByLabelText("20,000,000 tokens")).toBeTruthy();
  expect(view.container.querySelector(".usage-footer")?.textContent).toContain("Claude Sonnet 4.6");
  expect(screen.queryByLabelText("Providers")).toBeNull();
  fireEvent.focus(view.container.querySelector(".usage-column:last-of-type")!);
  expect(within(screen.getByRole("tooltip")).getByText("≈ $6.25 API equivalent")).toBeTruthy();
  expect(screen.getByRole("tooltip").textContent).toContain("Claude Opus 4.6");
  await select("period", "7 days");
  expect(screen.getByLabelText("8,000,000 tokens")).toBeTruthy();
  expect(view.container.querySelectorAll(".usage-column")).toHaveLength(7);
});

test("missing history stays unavailable and indexing does not become a zero", () => {
  render(<UsageOverview usage={{ ...unavailable, scanStatus: "indexing" }} />);
  expect(screen.getByText("Indexing…")).toBeTruthy();
  expect(screen.getByLabelText("Usage unavailable")).toBeTruthy();
  expect(screen.queryByRole("button", { name: /0 tokens/ })).toBeNull();
});
