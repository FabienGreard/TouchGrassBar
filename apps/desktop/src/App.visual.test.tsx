import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";

import {
  createBrowserSanitizedDesktopStateAdapter,
  type BrowserFixtureName,
} from "@/browserSanitizedDesktopStateAdapter";
import { DevFixtureSwitcher } from "@/components/dev-fixture-switcher";
import {
  DoomerboardPreview,
  TokenmaxxersEmpty,
} from "@/components/panel/doomerboard-preview";
import { PanelView } from "@/components/panel/panel-view";
import { OnboardingScreen } from "@/components/screens/onboarding-screen";
import { SettingsScreen } from "@/components/screens/settings-screen";
import {
  currentDoomerboardPreviewRows,
  currentUsagePreview,
  friendsDoomerboardPreviewRows,
} from "@/previewFixtures";
import { createSanitizedDesktopStateDelivery } from "@/sanitizedDesktopStateDelivery";

async function deliveredBrowserFixture(name: BrowserFixtureName) {
  const delivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      name,
      () => new Date("2026-08-03T00:00:00.000Z"),
    ),
  );

  let stop: (() => void) | undefined;
  const delivered = new Promise<void>((resolve, reject) => {
    stop = delivery.subscribe(() => {
      const view = delivery.getSnapshot();
      if (view.phase === "ready") resolve();
      if (view.phase === "degraded") reject(new Error("fixture unavailable"));
    });
  });
  await delivered;
  stop?.();

  const snapshot = delivery.getSnapshot().snapshot;
  if (snapshot === null) throw new Error("fixture unavailable");
  return snapshot;
}

describe("approved panel presentation contract", () => {
  test("offers a compact development fixture switcher", () => {
    const markup = renderToStaticMarkup(
      <DevFixtureSwitcher activeFixture="current" />,
    );

    expect(markup).toContain('aria-label="Development fixture"');
    expect(markup).toContain('href="?fixture=loading"');
    expect(markup).toContain('href="?fixture=unavailable"');
    expect(markup).toContain('href="?fixture=current"');
    expect(markup).toContain('href="?fixture=update"');
    expect(markup).toContain('href="?fixture=stale"');
    expect(markup).toContain('aria-current="page"');
    expect(markup).toContain("fixed");
    expect(markup).toContain("bg-menu-glass");
    expect(markup).not.toContain("bg-[#111713e8]");
  });

  test("keeps the complete unavailable panel stable", async () => {
    const unavailableState = await deliveredBrowserFixture("unavailable");
    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={unavailableState}
      />,
    );

    const normalizedMarkup = markup
      .replaceAll(/href="[^"]+"/g, 'href="[asset]"')
      .replaceAll(/src="[^"]+"/g, 'src="[asset]"');

    expect(normalizedMarkup).toMatchSnapshot();
    expect(markup).not.toContain(">0<");
    expect(markup).toContain('aria-label="Open panel menu"');
    expect(markup.match(/aria-label="Open panel menu"/g)).toHaveLength(1);
    expect(markup).toContain('data-icon-provider="hugeicons"');
    expect(markup).toContain("Doomerboard unavailable");
    expect(markup).toContain('aria-label="Current user identity unavailable"');
    expect(markup).not.toContain("— users");
    expect(markup).toContain('data-slot="doomerboard-viewport"');
    expect(markup).toContain("h-[180px]");
    expect(markup).toContain("Tokenmaxxers");
    expect(markup).not.toContain("My Tokenmaxxers");
    expect(markup).toContain("Global");
    expect(markup).toMatch(
      /aria-label="Select Doomerboard period"[^>]*>Today<\/button>/,
    );
    expect(markup).toContain("Combined");
    expect(markup).toContain('aria-label="Select Doomerboard period"');
    expect(markup).toContain('aria-label="Select Doomerboard provider"');
    expect(markup.match(/aria-expanded:bg-cream-ink\/5/g)).toHaveLength(3);
    expect(markup.match(/data-slot="metric-gauge"/g)).toHaveLength(3);
    expect(markup.match(/data-slot="provider-quota-lane"/g)).toHaveLength(2);
    expect(markup.match(/data-slot="quota-progress"/g)).toHaveLength(4);
    expect(markup.match(/>5-hour limit</g)).toHaveLength(2);
    expect(markup).not.toMatch(/>(?:laura|nora|max)<|#TG-/i);
    expect(markup).not.toContain('data-slot="skeleton"');
    expect(markup).not.toContain("ml-0.5");
    expect(markup).not.toContain("lucide");
    expect(markup).not.toContain("translate-y-px");
    expect(markup).toContain('data-icon-provider="hugeicons"');
    expect(markup).toContain('data-icon-tone="default"');
    expect(markup.match(/min-w-\[82px\]/g)).toHaveLength(2);
    expect(markup.match(/px-\[13px\]/g)).toHaveLength(3);
    expect(markup).toContain("w-[402px]");
  });

  test("ships contrast and motion adaptations", () => {
    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={true}
        state={null}
      />,
    );

    expect(markup).toContain('data-slot="dropdown-menu-trigger"');
    expect(markup).toContain('data-slot="loading-panel"');
    expect(markup).toContain('aria-busy="true"');
    expect(markup.match(/>Weekly limit</g)).toHaveLength(2);
    expect(markup.match(/>5-hour limit</g)).toHaveLength(2);
    expect(markup).toContain("Doomerboard unavailable");
    expect(markup.match(/data-slot="provider-quota-lane"/g)).toHaveLength(2);
    expect(markup.match(/data-slot="quota-progress"/g)).toHaveLength(4);
    expect(markup).not.toContain('data-slot="skeleton"');
    expect(markup).toContain("contrast-more:border-cream-ink");
    expect(markup).toContain("animate-pulse motion-reduce:animate-none");
  });

  test("shows the primary update action only when an update is available", async () => {
    const currentState = await deliveredBrowserFixture("current");
    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        onUpdate={() => undefined}
        refreshing={false}
        state={currentState}
        updateAvailable
      />,
    );

    expect(markup).toContain('data-slot="update-action"');
    expect(markup).toContain('data-variant="action"');
    expect(markup).toContain('aria-label="Download update"');
    expect(markup).toContain('data-size="icon"');
    expect(markup).toContain('data-icon-source="Download04Icon"');
    expect(markup).not.toContain(">Update</button>");
  });

  test("renders the populated development fixture without changing the production fallback", async () => {
    const currentState = await deliveredBrowserFixture("current");
    const markup = renderToStaticMarkup(
      <PanelView
        doomerboardPreviewRows={currentDoomerboardPreviewRows}
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={currentState}
        usagePreview={currentUsagePreview}
      />,
    );

    expect(markup).toContain('data-preview-fixture="doomerboard"');
    expect(markup).toContain('data-preview-fixture="usage"');
    expect(markup).toContain("laura");
    expect(markup).toContain("18.2M");
    expect(markup).toContain("#TG-4COLD7");
    expect(markup).toContain("#TG-LOOP55");
    expect(markup).toContain("#TG-7K4P9D");
    expect(markup).toContain("Fabien#TG-7K4P9D");
    expect(markup).toContain(
      'aria-label="Copy current user identity Fabien#TG-7K4P9D"',
    );
    expect(markup).toContain('data-copy-status="idle"');
    expect(markup).not.toContain("data-copy-indicator");
    expect(markup).toContain('data-copy-feedback="idle"');
    expect(markup).not.toContain(">Copied<");
    expect(markup).toContain('data-slot="current-user-identity"');
    expect(markup).not.toContain('data-slot="panel-identity-footer"');
    expect(markup).not.toContain("2nd");
    expect(markup).toContain('data-slot="segmented-control"');
    expect(markup).not.toContain("8 users");
    expect(markup).toContain("-8%");
    expect(markup).toContain("Down 8 percent from the previous day");
    expect(markup).toContain('data-tone="negative"');
    expect(markup).toContain("text-destructive");
    expect(markup).toContain("+14%");
    expect(markup).toContain("+22%");
    expect(markup).toContain("width:34%");
    expect(markup).toContain("width:64%");
    expect(markup).toContain("width:100%");
    expect(markup).toContain("Weekly limit · resets Mon 08:00");
    expect(markup).toContain("5-hour limit · resets 14:40");
    expect(markup).toContain("5-hour limit · resets 18:20");
    expect(markup).toContain('data-slot="provider-quota-lane"');
    expect(markup.match(/data-quota-tone="codex"/g)).toHaveLength(2);
    expect(markup.match(/data-quota-tone="claude"/g)).toHaveLength(2);
    expect(markup).toContain('data-quota-value="74"');
    expect(markup).toContain('data-quota-value="18"');
    expect(markup).toContain("--quota-codex-low");
    expect(markup).toContain("--quota-claude-low");
    expect(markup).toContain('data-slot="doomerboard-scroll"');
    expect(markup).toContain('data-slot="doomerboard-viewport"');
    expect(markup).toContain('data-slot="doomerboard-ledger"');
    expect(markup).toContain("overflow-y-auto");
    expect(markup).not.toMatch(/data-slot="doomerboard-scroll"[^>]*tabindex/);
    expect(markup).not.toMatch(/data-slot="doomerboard-ledger"[^>]*tabindex/);
    expect(markup).not.toContain("Doomerboard unavailable");
    expect(markup).not.toContain("My Tokenmaxxers");
  });

  test("offers an honest invitation state for an empty Tokenmaxxers board", () => {
    const markup = renderToStaticMarkup(<TokenmaxxersEmpty />);

    expect(markup).toContain("Your board is waiting");
    expect(markup).toContain("Invite your friends to join your tokenmaxxers");
    expect(markup).toContain("Invite a friend");
    expect(markup).toContain('data-icon-provider="hugeicons"');
    expect(markup).toContain('data-icon-tone="default"');
    expect(markup).not.toContain("lucide");
    expect(markup).toContain("h-full");
    expect(markup).not.toMatch(/<button[^>]*\sdisabled(?:=|>)/);
  });

  test("offers a populated fake Tokenmaxxers development mockup", () => {
    const markup = renderToStaticMarkup(
      <DoomerboardPreview
        initialAudience="mine"
        previewRows={currentDoomerboardPreviewRows}
        tokenmaxxerPreviewRows={friendsDoomerboardPreviewRows}
      />,
    );

    expect(markup).toContain('aria-label="Tokenmaxxers preview fixture"');
    expect(markup).toContain("TOUCH GRASS?");
    expect(markup).toContain("STILL ONLINE");
    expect(markup).toContain("Fabien");
    expect(markup).not.toContain("Your board is waiting");
  });

  test("uses the approved native-sheet composition for onboarding", () => {
    const markup = renderToStaticMarkup(<OnboardingScreen />);

    expect(markup).toContain('data-slot="native-window"');
    expect(markup).toContain("Confirm your providers.");
    expect(markup.match(/data-slot="provider-mark"/g)).toHaveLength(2);
    expect(markup).toContain("Provider status unavailable");
    expect(markup).toMatch(/<button[^>]*disabled=""[^>]*>Continue<\/button>/);
    expect(markup).toContain("Prompts and conversations never leave this Mac.");
  });

  test("uses the approved native-sheet composition for settings", () => {
    const markup = renderToStaticMarkup(<SettingsScreen />);

    expect(markup).toContain('data-slot="native-window"');
    expect(markup).toContain('aria-current="page"');
    expect(markup).toContain("General");
    expect(markup).toContain("Launch at login");
    expect(markup).toContain('data-slot="switch"');
    expect(markup).not.toContain("Control the spiral.");
  });
});
