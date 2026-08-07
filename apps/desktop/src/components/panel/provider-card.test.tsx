import { renderToStaticMarkup } from "react-dom/server";
import type { ProviderPresentation } from "@touchgrass/contracts";
import { describe, expect, test } from "vitest";

import { ProviderCard } from "@/components/panel/provider-card";

function unavailableProvider(scanStatus: "indexing" | "unavailable") {
  return {
    displayName: "Claude",
    presence: "not-detected",
    provider: "claude",
    quota: {
      availability: "unavailable",
      provider: "claude",
      quotaLanes: [],
    },
    usage: {
      scanStatus,
      sevenDays: { availability: "unavailable" },
      thirtyDays: { availability: "unavailable" },
      today: { availability: "unavailable" },
    },
  } as const satisfies ProviderPresentation;
}

describe("provider card", () => {
  test("pulses only the provider row while it reconnects", () => {
    const markup = renderToStaticMarkup(
      <ProviderCard presentation={unavailableProvider("indexing")} />,
    );

    expect(markup).toContain('aria-busy="true"');
    expect(markup).toContain("animate-pulse motion-reduce:animate-none");
    expect(markup).toContain("Refreshing Claude…");
  });

  test("does not pulse an ordinary unavailable provider", () => {
    const markup = renderToStaticMarkup(
      <ProviderCard presentation={unavailableProvider("unavailable")} />,
    );

    expect(markup).not.toContain("aria-busy");
    expect(markup).not.toContain("animate-pulse");
    expect(markup).not.toContain("Refreshing Claude…");
  });
});
