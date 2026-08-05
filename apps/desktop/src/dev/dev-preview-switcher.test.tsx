import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, test, vi } from "vitest";

import { DevInstanceBadge } from "@/dev/dev-instance-badge";
import { resolveDevInstance } from "@/dev/dev-instance";
import { DevFixtureSwitcher } from "@/dev/dev-preview-switcher";

afterEach(() => vi.unstubAllGlobals());

describe("development preview switcher", () => {
  test("offers compact panel fixture controls", () => {
    const devInstance = resolveDevInstance({
      branch: "agent/issue-47-identify-dev-instances",
      worktreeSeed: "semantic-proof",
    });
    const markup = renderToStaticMarkup(
      <DevFixtureSwitcher
        activeFixture="current"
        devInstance={devInstance}
      />,
    );
    const badges = (["panel", "settings", "onboarding"] as const).map(
      (surface) =>
        renderToStaticMarkup(
          <DevInstanceBadge instance={devInstance} surface={surface} />,
        ),
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
    expect(markup).toContain("fixed");
    expect(markup).toContain("bg-menu-glass");
    expect(markup).not.toContain("bg-[#111713e8]");
    expect(badges).toHaveLength(3);
    for (const badge of badges) {
      expect(badge).toContain(
        'aria-label="Development instance #47 Identify dev instances"',
      );
      expect(badge).toContain('data-slot="dev-instance-badge"');
      expect(badge).toContain("#47 Identify dev instances");
    }
    expect(badges[0]).toContain('data-dev-instance-surface="panel"');
    expect(badges[1]).toContain('data-dev-instance-surface="settings"');
    expect(badges[2]).toContain('data-dev-instance-surface="onboarding"');
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
});
