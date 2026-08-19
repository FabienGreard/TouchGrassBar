import {
  CONTRACT_VERSION,
  type SanitizedDesktopState,
} from "@touchgrass/contracts";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";

import { PanelHeader } from "@/components/panel/panel-header";
import { refreshActionLabel } from "@/components/panel/panel-header-copy";
import { PanelView } from "@/components/panel/panel-view";

type PanelSyncStatus = SanitizedDesktopState["sync"]["status"];

function unavailableUsage() {
  return {
    scanStatus: "unavailable" as const,
    sevenDays: { availability: "unavailable" as const },
    thirtyDays: { availability: "unavailable" as const },
    today: { availability: "unavailable" as const },
  };
}

function stateWithSync(
  status: PanelSyncStatus,
  providerAvailability: "current" | "stale" = "current",
): SanitizedDesktopState {
  const observedAt = "2026-08-08T12:00:00.000Z";

  return {
    combinedUsage: unavailableUsage(),
    contractVersion: CONTRACT_VERSION,
    generatedAt: observedAt,
    profile: { status: "not-authorized" },
    providers: [
      {
        displayName: "Codex",
        presence: "detected",
        provider: "codex",
        quota: {
          availability: providerAvailability,
          observedAt,
          provider: "codex",
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
        usage: unavailableUsage(),
      },
    ],
    revision: "1",
    sync: {
      lastSuccessfulAt: status === "synced" ? observedAt : null,
      status,
    },
  };
}

function renderHeader({
  error = false,
  refreshing = false,
  state = stateWithSync("synced"),
}: {
  error?: boolean;
  refreshing?: boolean;
  state?: SanitizedDesktopState | null;
} = {}) {
  return renderToStaticMarkup(
    <PanelHeader
      error={error}
      onAddTokenmaxxer={() => undefined}
      onRefresh={() => undefined}
      onSettings={() => undefined}
      onUpdate={() => undefined}
      refreshing={refreshing}
      state={state}
      updateActionLabel={null}
    />,
  );
}

describe("panel sync status", () => {
  test.each([
    "synced",
    "pending",
    "stale",
    "offline",
    "authority-rejected",
    "unavailable",
  ] as const)("keeps the loaded %s state safe", (status) => {
    const markup = renderHeader({ state: stateWithSync(status) });

    expect(markup).toContain(`data-sync-status="${status}"`);
    expect(markup).toContain('aria-live="polite"');
    if (status === "pending") {
      expect(markup).toContain(">Syncing…</small>");
    } else {
      expect(markup).toContain(">Live");
    }
  });

  test.each([
    ["stale", "Synchronization is delayed"],
    ["offline", "Synchronization is offline"],
    ["authority-rejected", "Active Mac transferred"],
  ] as const)(
    "reports the loaded %s detail outside the headline",
    (status, detail) => {
      const markup = renderHeader({ state: stateWithSync(status) });

      expect(markup).toContain('>Live<span class="sr-only">. ');
      expect(markup).toContain(detail);
    },
  );

  test("keeps sync status independent from provider freshness", () => {
    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={stateWithSync("synced", "stale")}
      />,
    );

    expect(markup).toContain('data-provider-availability="stale"');
    expect(markup).toContain('data-sync-status="synced"');
    expect(markup).toContain(">Live</small>");
    expect(markup).not.toContain("Sync is stale");
  });

  test("shows syncing during a refresh", () => {
    const markup = renderHeader({
      error: true,
      refreshing: true,
      state: stateWithSync("authority-rejected"),
    });

    expect(markup).toContain('data-sync-status="authority-rejected"');
    expect(markup).toContain(">Syncing…</small>");
    expect(markup).not.toContain(">Sync unavailable</small>");
  });

  test("uses unavailable only when no state can load", () => {
    expect(renderHeader({ error: true, state: null })).toContain(
      ">Sync unavailable</small>",
    );
    const loaded = renderHeader({
      error: true,
      state: stateWithSync("unavailable"),
    });
    expect(loaded).toContain(">Live</small>");
    expect(loaded).not.toContain("Synchronization is unavailable");
  });

  test("uses refresh copy for the broad provider action", () => {
    expect(refreshActionLabel(false)).toBe("Refresh now");
    expect(refreshActionLabel(true)).toBe("Refreshing…");
  });

  test("does not render private sync diagnostics", () => {
    const state = {
      ...stateWithSync("authority-rejected"),
      sync: {
        credential: "sentinel-credential",
        installationId: "sentinel-installation",
        rawResponse: "sentinel-response",
        session: "sentinel-session",
        status: "authority-rejected",
        transportError: "sentinel-transport",
      },
    } as unknown as SanitizedDesktopState;

    const markup = renderHeader({ state });

    expect(markup).toContain(">Live<span");
    expect(markup).toContain("Active Mac transferred");
    expect(markup).not.toContain("sentinel-");
  });
});
