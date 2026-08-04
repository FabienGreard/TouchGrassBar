import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";

import { createBrowserSanitizedDesktopStateAdapter } from "@/dev/browser-sanitized-desktop-state-adapter";
import type { BrowserFixtureName } from "@/dev/preview-scenario";
import { Doomerboard, TokenmaxxersEmpty } from "@/components/panel/doomerboard";
import { PanelView } from "@/components/panel/panel-view";
import {
  currentProfile,
  currentDoomerboardRows,
  currentUsagePresentation,
  myTokenmaxxerRows,
} from "@/dev/panel-fixtures";
import { createSanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";

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
    expect(markup).toContain('data-slot="brand-mark"');
    expect(markup).toContain('data-tone="color"');
    expect(markup).not.toMatch(
      /<img[^>]*brightness-0[^>]*data-slot="brand-mark"/,
    );
    expect(markup.match(/aria-label="Open panel menu"/g)).toHaveLength(1);
    expect(markup).toContain('data-icon-provider="hugeicons"');
    expect(markup).toContain("Doomerboard unavailable");
    expect(markup).toContain('aria-label="Current user profile unavailable"');
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
    expect(markup.match(/aria-expanded:bg-pearl-ink\/5/g)).toHaveLength(3);
    expect(markup.match(/data-slot="metric-gauge"/g)).toHaveLength(3);
    expect(markup).not.toContain('data-slot="provider-quota-lane"');
    expect(markup.match(/data-slot="quota-progress"/g)).toHaveLength(2);
    expect(markup).not.toContain("Weekly limit");
    expect(markup).not.toContain("5-hour limit");
    expect(
      markup.match(/data-provider-availability="unavailable"/g),
    ).toHaveLength(2);
    expect(markup).not.toContain("1970-01-01");
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
    expect(markup).not.toContain("Weekly limit");
    expect(markup).not.toContain("5-hour limit");
    expect(
      markup.match(/data-provider-availability="unavailable"/g),
    ).toHaveLength(2);
    expect(markup).not.toContain("1970-01-01");
    expect(markup).toContain("Doomerboard unavailable");
    expect(markup).not.toContain('data-slot="provider-quota-lane"');
    expect(markup.match(/data-slot="quota-progress"/g)).toHaveLength(2);
    expect(markup).not.toContain('data-slot="skeleton"');
    expect(markup).toContain("contrast-more:border-pearl-ink");
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
    expect(markup).toContain('data-variant="primary"');
    expect(markup).toContain('aria-label="Download update"');
    expect(markup).toContain('data-size="icon"');
    expect(markup).toContain('data-icon-source="Download04Icon"');
    expect(markup).not.toContain(">Update</button>");
  });

  test("renders the populated development fixture without changing the production fallback", async () => {
    const currentState = await deliveredBrowserFixture("current");
    const markup = renderToStaticMarkup(
      <PanelView
        currentProfile={currentProfile}
        doomerboardRows={currentDoomerboardRows}
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={currentState}
        usagePresentation={currentUsagePresentation}
      />,
    );

    expect(markup).not.toContain("data-preview-fixture");
    expect(markup).toContain("laura");
    expect(markup).toContain("18.2M");
    expect(markup).toContain("#TG-4COLD7");
    expect(markup).toContain("#TG-LOOP55");
    expect(markup).toContain("#TG-7K4P9D");
    expect(markup).toContain("Fabien#TG-7K4P9D");
    expect(markup).toContain(
      'aria-label="Copy current user profile Fabien#TG-7K4P9D"',
    );
    expect(markup).toContain('data-copy-status="idle"');
    expect(markup).not.toContain("data-copy-indicator");
    expect(markup).toContain('data-copy-feedback="idle"');
    expect(markup).not.toContain(">Copied<");
    expect(markup).toContain('data-slot="current-user-profile"');
    expect(markup).not.toContain('data-slot="panel-profile-footer"');
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
    expect(markup).toContain('data-slot="scroll-area-root"');
    expect(markup).toContain('data-slot="scroll-area-viewport"');
    expect(markup).toContain('data-slot="doomerboard-viewport"');
    expect(markup).toContain('data-slot="doomerboard-ledger"');
    expect(markup).toContain("data-radix-scroll-area-viewport");
    expect(markup).not.toMatch(/data-doomerboard-scroll=""[^>]*tabindex/);
    expect(markup).not.toMatch(/data-slot="doomerboard-ledger"[^>]*tabindex/);
    expect(markup).not.toContain("Doomerboard unavailable");
    expect(markup).not.toContain("My Tokenmaxxers");
  });

  test("offers an honest invitation state for an empty Tokenmaxxers board", () => {
    const markup = renderToStaticMarkup(<TokenmaxxersEmpty />);

    expect(markup).toContain("Your board is waiting");
    expect(markup).toContain("Add people by TouchGrass ID to compare scores.");
    expect(markup).toContain("Add a Tokenmaxxer");
    expect(markup).toContain('data-icon-provider="hugeicons"');
    expect(markup).toContain('data-icon-tone="default"');
    expect(markup).not.toContain("lucide");
    expect(markup).toContain("h-full");
    expect(markup).not.toMatch(/<button[^>]*\sdisabled(?:=|>)/);
  });

  test("offers a populated fake Tokenmaxxers development mockup", () => {
    const markup = renderToStaticMarkup(
      <Doomerboard
        currentProfile={currentProfile}
        initialAudience="mine"
        rows={currentDoomerboardRows}
        tokenmaxxerRows={myTokenmaxxerRows}
      />,
    );

    expect(markup).toContain('aria-label="Tokenmaxxers rankings"');
    expect(markup).toContain("TOUCH GRASS?");
    expect(markup).toContain("STILL ONLINE");
    expect(markup).toContain("Fabien");
    expect(markup).not.toContain("Your board is waiting");
  });

  test("does not infer the current Profile from fixture conventions", () => {
    const markup = renderToStaticMarkup(
      <Doomerboard rows={currentDoomerboardRows} />,
    );

    expect(markup).toContain('aria-label="Current user profile unavailable"');
    expect(markup).not.toContain("Copy current user profile");
  });
});
