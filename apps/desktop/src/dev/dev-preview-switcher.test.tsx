import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";

import { DevFixtureSwitcher } from "@/dev/dev-preview-switcher";

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
});
