import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, test, vi } from "vitest";

import { applyDevInstanceDocument } from "@/dev/dev-instance-document";
import { resolveDevInstance } from "@/dev/dev-instance";
import {
  DevFixtureSwitcher,
  DevPreviewSwitcher,
} from "@/dev/dev-preview-switcher";

afterEach(() => vi.unstubAllGlobals());

describe("development preview switcher", () => {
  test("presents the development identity and compact panel controls", () => {
    const devInstance = resolveDevInstance({
      branch: "agent/issue-47-identify-dev-instances",
      worktreeSeed: "semantic-proof",
    });
    const setProperty = vi.fn();
    vi.stubGlobal("document", {
      documentElement: { dataset: {}, style: { setProperty } },
      title: "",
    });
    applyDevInstanceDocument(devInstance, "panel");
    const markup = renderToStaticMarkup(
      <DevFixtureSwitcher
        activeFixture="current"
        devInstance={devInstance}
      />,
    );

    expect(markup).toContain('aria-label="Development fixture"');
    expect(markup).toContain('href="?fixture=loading"');
    expect(markup).toContain('href="?fixture=unavailable"');
    expect(markup).toContain('href="?fixture=current"');
    expect(markup).toContain('href="?fixture=update"');
    expect(markup).toContain('href="?fixture=stale"');
    expect(markup).toContain('aria-current="page"');
    expect(markup).toContain('aria-label="Drag development preview"');
    expect(markup).toContain('aria-label="Expand development preview"');
    expect(markup).toContain('data-icon-source="ArrowExpand01Icon"');
    expect(markup).not.toContain('aria-label="Hide development preview"');
    expect(markup).toContain('data-state="minimized"');
    expect(markup).toContain("Preview · #47 Identify dev instances");
    expect(document.title).toBe(
      "TouchGrassBar · #47 Identify dev instances",
    );
    expect(document.documentElement.dataset.devInstance).toBe(
      devInstance.instanceKey,
    );
    expect(setProperty).toHaveBeenCalledWith(
      "--dev-instance-accent",
      expect.any(String),
    );
    expect(markup).toContain("fixed");
    expect(markup).toContain("bg-menu-glass");
    expect(markup).not.toContain("bg-[#111713e8]");
  });

  test("restores its expanded state and position after navigation", () => {
    const getItem = vi.fn(() =>
      JSON.stringify({
        mode: "expanded",
        position: { left: 88, top: 96 },
      }),
    );
    vi.stubGlobal("window", { sessionStorage: { getItem } });

    const markup = renderToStaticMarkup(
      <DevFixtureSwitcher activeFixture="current" />,
    );

    expect(getItem).toHaveBeenCalledWith(
      "touchgrass:dev-preview-panel-state",
    );
    expect(markup).toContain('data-state="expanded"');
    expect(markup).toContain(
      'style="bottom:auto;left:88px;right:auto;top:96px"',
    );
    expect(markup).toContain('aria-label="Minimize development preview"');
  });

  test("switches the Settings update preview without losing its context", () => {
    const markup = renderToStaticMarkup(
      <DevPreviewSwitcher
        activeFixture="update"
        activeSurface="settings"
        settingsProviderPreviewState="not-installed"
      />,
    );

    expect(markup).toContain('aria-label="Update preview states"');
    expect(markup).toContain(
      'href="?window=settings&amp;fixture=current&amp;providerState=not-installed#settings-general"',
    );
    expect(markup).toContain(
      'href="?window=settings&amp;fixture=update&amp;providerState=not-installed#settings-general"',
    );
    expect(markup).toContain('aria-current="page"');
    expect(markup).toContain("No update");
    expect(markup).toContain("Available");
  });
});
