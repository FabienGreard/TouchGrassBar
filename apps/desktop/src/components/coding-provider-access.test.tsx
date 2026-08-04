import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";

import { CodingProviderAccessCard } from "@/components/coding-provider-access";
import type { CodingProviderAccessState } from "@/components/coding-provider-access-state";

const states = [
  {
    action: null,
    copy: "Provider detection is not connected in this build.",
    detail: null,
    label: "Unavailable",
    state: "unavailable",
  },
  {
    action: "Check again",
    copy: "Codex was detected on this Mac.",
    detail: null,
    label: "Detected",
    state: "detected",
  },
  {
    action: "Check now",
    copy: "Detected locally and reporting provider limits.",
    detail: null,
    label: "Ready",
    state: "ready",
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
          onCheck={() => undefined}
          onViewInstallationSteps={() => undefined}
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
});
