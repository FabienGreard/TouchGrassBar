import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, test, vi } from "vitest";

import { DevFixtureSwitcher } from "@/dev/dev-preview-switcher";

afterEach(() => vi.unstubAllGlobals());

describe("development preview switcher", () => {
  test("offers compact panel fixture controls", () => {
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
    expect(markup).toContain('aria-label="Drag development preview"');
    expect(markup).toContain('aria-label="Expand development preview"');
    expect(markup).toContain('data-icon-source="ArrowExpand01Icon"');
    expect(markup).not.toContain('aria-label="Hide development preview"');
    expect(markup).toContain('data-state="minimized"');
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
});
