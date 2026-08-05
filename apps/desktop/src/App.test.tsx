import { afterEach, describe, expect, test, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

import { App } from "@/App";

function renderSettings(hash: string) {
  vi.stubGlobal("window", {
    history: { replaceState: vi.fn() },
    location: { hash },
  });
  return renderToStaticMarkup(<App hasNativeRuntime surface="settings" />);
}

afterEach(() => vi.unstubAllGlobals());

describe("native App composition", () => {
  test("does not invent provider detection in production Settings", () => {
    const markup = renderSettings("#settings-providers");

    expect(
      markup.match(/data-coding-provider-access-state="unavailable"/g),
    ).toHaveLength(2);
    expect(markup).not.toContain('data-coding-provider-access-state="ready"');
    expect(markup).not.toContain(
      'data-coding-provider-access-state="not-installed"',
    );
    expect(markup).not.toContain("Detected locally");
    expect(markup).not.toContain("was not found");
    expect(markup).not.toContain("Check now");
    expect(markup).not.toContain("Check again");
    expect(markup).not.toContain("data-dev-instance");
  });

  test("does not invent a Profile in production Settings", () => {
    const markup = renderSettings("#settings-profile");

    expect(markup).toContain('data-profile-state="unavailable"');
    expect(markup).not.toContain("Fabien");
    expect(markup).not.toContain("#TG-");
    expect(markup).not.toContain('data-profile-state="saved"');
    expect(markup).not.toContain(">Edit<");
    expect(markup).not.toContain("Copy ID");
  });

  test("keeps disconnected production Settings controls inert", () => {
    const markup = renderSettings("#settings-general");

    expect(markup.match(/role="switch"[^>]*disabled=""/g)).toHaveLength(2);
    expect(markup).not.toContain('role="switch" aria-checked="true"');
    expect(markup).toMatch(/<button[^>]*disabled=""[^>]*>Check now/);
  });

  test("does not invent provider detection in production onboarding", () => {
    const markup = renderToStaticMarkup(
      <App hasNativeRuntime surface="onboarding" />,
    );

    expect(
      markup.match(/data-coding-provider-access-state="unavailable"/g),
    ).toHaveLength(2);
    expect(markup).not.toContain('data-coding-provider-access-state="ready"');
    expect(markup).not.toContain(
      'data-coding-provider-access-state="not-installed"',
    );
  });
});
