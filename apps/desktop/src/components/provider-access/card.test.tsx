import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";

import { CodingProviderAccessCard } from "@/components/provider-access/card";
import type { CodingProviderAccessState } from "@/components/provider-access/presentation";

const states = [
  {
    action: "Check again",
    copy: "TouchGrassBar could not check Codex on this Mac. It will try again.",
    detail: null,
    label: "Unavailable",
    state: "unavailable",
  },
  {
    action: "Check again",
    copy: "Codex was detected on this Mac.",
    detail: null,
    label: "Ready",
    state: "detected",
  },
  {
    action: "Check again",
    copy: "Codex is installed, but TouchGrassBar cannot read its local state yet.",
    detail: "Finish local access",
    label: "Needs access",
    state: "needs-access",
  },
  {
    action: "Check again",
    copy: "Codex was not found in Applications or your command-line tools.",
    detail: "Connect Codex",
    label: "Not installed",
    state: "not-installed",
  },
] satisfies ReadonlyArray<{
  action: string | null;
  copy: string;
  detail: string | null;
  label: string;
  state: CodingProviderAccessState;
}>;

describe("coding provider access", () => {
  test("presents every provider state at the component seam", () => {
    for (const state of states) {
      const markup = renderToStaticMarkup(
        <CodingProviderAccessCard
          displayName="Codex"
          onCheck={() => undefined}
          provider="codex"
          state={state.state}
        />,
      );

      expect(markup).toContain(
        `data-coding-provider-access-state="${state.state}"`,
      );
      expect(markup).toContain(`>${state.label}<`);
      expect(markup).toContain(state.copy);
      if (state.detail === null) {
        expect(markup).not.toContain("Finish local access");
        expect(markup).not.toContain("Connect Codex");
      } else {
        expect(markup).toContain(state.detail);
      }
      if (state.action === null) {
        expect(markup).not.toContain("Check now");
        expect(markup).not.toContain("Check again");
      } else {
        expect(markup).toContain(`>${state.action}<`);
      }
    }
  });

  test("keeps the Settings control alone at the top-right", () => {
    const markup = renderToStaticMarkup(
      <CodingProviderAccessCard
        displayName="Claude"
        enabled
        onCheck={() => undefined}
        onEnabledChange={() => undefined}
        provider="claude"
        state="detected"
      />,
    );

    expect(markup).toContain("absolute right-5 top-5");
    expect(markup).toContain(
      'aria-label="Show Claude and include its quota and usage in totals"',
    );
    expect(markup).not.toContain("Show and include");
    expect(markup).toContain("mt-auto flex items-center justify-end");
    expect(markup).toContain("pt-1");
    expect(markup).not.toContain("pt-2.5");
    expect(markup).not.toContain("absolute bottom-4 right-5");
    expect(markup).toContain('class="-mr-9 flex h-5 items-center"');
    expect(markup).toContain('data-slot="provider-action-spacer"');
    expect(markup).toContain('aria-label="Check Claude again"');
    expect(markup).toContain("-mr-1.5 mb-1");
  });

  test("keeps Ready, Excluded, and Unavailable compact", () => {
    const enabled = renderToStaticMarkup(
      <CodingProviderAccessCard
        displayName="Claude"
        enabled
        onCheck={() => undefined}
        onEnabledChange={() => undefined}
        provider="claude"
        state="not-installed"
      />,
    );
    const excluded = renderToStaticMarkup(
      <CodingProviderAccessCard
        displayName="Claude"
        enabled={false}
        onCheck={() => undefined}
        onEnabledChange={() => undefined}
        provider="claude"
        state="not-installed"
      />,
    );
    const unavailable = renderToStaticMarkup(
      <CodingProviderAccessCard
        displayName="Claude"
        enabled
        onCheck={() => undefined}
        onEnabledChange={() => undefined}
        provider="claude"
        state="unavailable"
      />,
    );
    const ready = renderToStaticMarkup(
      <CodingProviderAccessCard
        displayName="Claude"
        enabled
        onEnabledChange={() => undefined}
        provider="claude"
        state="detected"
      />,
    );

    expect(enabled).toContain("min-h-[188px]");
    expect(excluded).toContain("h-[108px]");
    expect(unavailable).toContain("h-[108px]");
    expect(unavailable).toContain('aria-label="Check Claude again"');
    expect(ready).toContain("h-[108px]");
    expect(excluded).toContain('data-slot="provider-action-spacer"');
    expect(excluded).not.toContain("Connect Claude");
    expect(excluded).not.toContain('aria-label="Check Claude again"');
  });

  test("keeps expanded card actions below their detail panel", () => {
    for (const state of ["needs-access", "not-installed"] as const) {
      const markup = renderToStaticMarkup(
        <CodingProviderAccessCard
          displayName="Claude"
          enabled
          onCheck={() => undefined}
          onEnabledChange={() => undefined}
          provider="claude"
          state={state}
        />,
      );

      expect(markup).toContain("min-h-[188px]");
      expect(markup).toContain(
        'class="-mr-9 mt-1.5 flex h-5 items-center"',
      );
      expect(markup).toContain('data-slot="provider-expanded-action"');
      expect(markup).not.toContain(
        'aria-label="Check Claude again" class="absolute bottom-4 right-5"',
      );
    }
  });

  test("links each missing provider to its official installation guide", () => {
    const claude = renderToStaticMarkup(
      <CodingProviderAccessCard
        displayName="Claude"
        provider="claude"
        state="not-installed"
      />,
    );
    const codex = renderToStaticMarkup(
      <CodingProviderAccessCard
        displayName="Codex"
        provider="codex"
        state="not-installed"
      />,
    );

    expect(claude).toContain(
      'href="https://docs.anthropic.com/en/docs/claude-code/getting-started"',
    );
    expect(codex).toContain(
      'href="https://developers.openai.com/codex/cli/"',
    );
    expect(claude).toContain('target="_blank"');
    expect(claude).toContain("Official installation guide");
  });
});
