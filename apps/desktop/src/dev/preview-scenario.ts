import type { DesktopSurface } from "@/App";
import {
  codingProviderAccessStates,
  type CodingProviderAccessState,
} from "@/components/coding-provider-access-state";
import {
  onboardingSteps,
  type OnboardingStep,
} from "@/components/screens/onboarding/onboarding-flow";

type BrowserFixtureName =
  "current" | "loading" | "stale" | "update" | "unavailable";

type DevPreviewScenario = {
  fixture: BrowserFixtureName;
  onboarding: {
    codexState: CodingProviderAccessState;
    initialStep: OnboardingStep;
    providerState: CodingProviderAccessState;
  };
  settingsProviderState: CodingProviderAccessState;
  surface: DesktopSurface;
};

function resolveFixture(params: URLSearchParams): BrowserFixtureName {
  const candidate = params.get("fixture");
  return candidate === "current" ||
    candidate === "loading" ||
    candidate === "stale" ||
    candidate === "update"
    ? candidate
    : "unavailable";
}

function resolveProviderState(
  params: URLSearchParams,
  key: "codexState" | "providerState",
  fallback: CodingProviderAccessState,
): CodingProviderAccessState {
  const candidate = params.get(key);
  return codingProviderAccessStates.some(
    ({ key: state }) => state === candidate,
  )
    ? (candidate as CodingProviderAccessState)
    : fallback;
}

function resolveOnboardingStep(params: URLSearchParams): OnboardingStep {
  const candidate = params.get("onboardingStep");
  return onboardingSteps.some(({ key }) => key === candidate)
    ? (candidate as OnboardingStep)
    : "providers";
}

function resolveSurface(params: URLSearchParams): DesktopSurface {
  const candidate = params.get("window");
  return candidate === "settings" || candidate === "onboarding"
    ? candidate
    : "panel";
}

function resolveBrowserFixtureName(search: string): BrowserFixtureName {
  return resolveFixture(new URLSearchParams(search));
}

function resolveDevPreviewScenario(search: string): DevPreviewScenario {
  const params = new URLSearchParams(search);
  const providerState = resolveProviderState(
    params,
    "providerState",
    "not-installed",
  );

  return {
    fixture: resolveFixture(params),
    onboarding: {
      codexState: resolveProviderState(params, "codexState", "ready"),
      initialStep: resolveOnboardingStep(params),
      providerState,
    },
    settingsProviderState: providerState,
    surface: resolveSurface(params),
  };
}

export { resolveBrowserFixtureName, resolveDevPreviewScenario };
export type { BrowserFixtureName, DevPreviewScenario };
