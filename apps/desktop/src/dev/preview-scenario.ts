import type { DesktopSurface } from "@/App";
import {
  codingProviderAccessStates,
  type CodingProviderAccessState,
} from "@/components/provider-access/presentation";
import {
  onboardingSteps,
  type OnboardingStep,
} from "@/components/screens/onboarding/onboarding-flow";

type BrowserFixtureName =
  "current" | "loading" | "stale" | "update" | "unavailable";

type OnboardingSetupPreviewState =
  "profile-pending" | "ready" | "required" | "unavailable";
type SettingsProfilePreviewState = "profile-pending" | "saved";

type DevPreviewScenario = {
  fixture: BrowserFixtureName;
  onboarding: {
    codexState: CodingProviderAccessState;
    initialStep: OnboardingStep;
    providerState: CodingProviderAccessState;
    setupState: OnboardingSetupPreviewState;
  };
  settingsProfileState: SettingsProfilePreviewState;
  settingsProviderEnabled: boolean;
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

function resolveOnboardingSetupState(
  params: URLSearchParams,
): OnboardingSetupPreviewState {
  const candidate = params.get("setupState");
  return candidate === "profile-pending" ||
    candidate === "required" ||
    candidate === "unavailable"
    ? candidate
    : "ready";
}

function resolveSurface(params: URLSearchParams): DesktopSurface {
  const candidate = params.get("window");
  return candidate === "settings" || candidate === "onboarding"
    ? candidate
    : "panel";
}

function resolveSettingsProfileState(
  params: URLSearchParams,
): SettingsProfilePreviewState {
  return params.get("profileState") === "profile-pending"
    ? "profile-pending"
    : "saved";
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
  const settingsProviderExcluded =
    params.get("providerState") === "excluded";

  return {
    fixture: resolveFixture(params),
    onboarding: {
      codexState: resolveProviderState(params, "codexState", "detected"),
      initialStep: resolveOnboardingStep(params),
      providerState,
      setupState: resolveOnboardingSetupState(params),
    },
    settingsProfileState: resolveSettingsProfileState(params),
    settingsProviderEnabled: !settingsProviderExcluded,
    settingsProviderState: providerState,
    surface: resolveSurface(params),
  };
}

export { resolveBrowserFixtureName, resolveDevPreviewScenario };
export type {
  BrowserFixtureName,
  DevPreviewScenario,
  OnboardingSetupPreviewState,
  SettingsProfilePreviewState,
};
