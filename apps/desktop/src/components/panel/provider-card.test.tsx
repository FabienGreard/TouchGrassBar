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

function cachedProvider() {
  return {
    ...unavailableProvider("indexing"),
    presence: "detected",
    quota: {
      availability: "current",
      observedAt: "2026-08-08T12:00:00Z",
      provider: "claude",
      quotaLanes: [
        {
          allowance: 100,
          label: "Weekly limit",
          remaining: 50,
          resetAt: null,
          unit: "percent",
        },
      ],
    },
  } as const satisfies ProviderPresentation;
}

function staleExpiredProvider() {
  return {
    ...cachedProvider(),
    quota: {
      ...cachedProvider().quota,
      availability: "stale",
      quotaLanes: [
        {
          allowance: 100,
          label: "Weekly limit",
          remaining: 50,
          resetAt: "2026-08-08T11:00:00Z",
          unit: "percent",
        },
      ],
    },
  } as const satisfies ProviderPresentation;
}

function cachedUsageProvider() {
  return {
    ...unavailableProvider("indexing"),
    presence: "detected",
    usage: {
      ...unavailableProvider("indexing").usage,
      today: {
        availability: "current",
        coverage: "complete",
        evidenceBasis: "locally-derived",
        observedAt: "2026-08-08T12:00:00Z",
        observedTokens: 60,
      },
    },
  } as const satisfies ProviderPresentation;
}

describe("provider card", () => {
  test("pulses only the provider row while usage is indexing", () => {
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

  test("shows cached quota without a loading pulse", () => {
    const markup = renderToStaticMarkup(<ProviderCard presentation={cachedProvider()} />);

    expect(markup).not.toContain("aria-busy");
    expect(markup).not.toContain("animate-pulse");
    expect(markup).toContain("Weekly limit");
    expect(markup).toContain("50%");
  });

  test("shows stale expired quota without exposing cache state", () => {
    const markup = renderToStaticMarkup(
      <ProviderCard
        presentation={staleExpiredProvider()}
        referenceTime="2026-08-08T12:00:00Z"
        timeZone="UTC"
      />,
    );

    expect(markup).toContain("Weekly limit");
    expect(markup).toContain("Weekly limit · reset Sat 8 Aug, 11:00");
    expect(markup).toContain("50%");
    expect(markup).not.toContain(" · stale");
    expect(markup).not.toContain("quota stale");
    expect(markup).not.toContain("0m left");
    expect(markup).not.toContain("expired");
  });

  test("does not show a loading pulse when only cached Observed Usage is available", () => {
    const markup = renderToStaticMarkup(<ProviderCard presentation={cachedUsageProvider()} />);

    expect(markup).not.toContain("aria-busy");
    expect(markup).not.toContain("animate-pulse");
    expect(markup).not.toContain("Refreshing Claude…");
  });
});
