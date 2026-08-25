import type { SyncStatus, UpdateState } from "@touchgrass/contracts";

import type { DesktopSurface } from "@/App";
import {
  codingProviderAccessStates,
  type CodingProviderAccessState,
} from "@/components/provider-access/presentation";
import {
  onboardingSteps,
  type OnboardingStep,
} from "@/components/screens/onboarding/onboarding-flow";

type BrowserFixtureName = "current" | "loading" | "stale" | "update" | "unavailable";
type UpdatePreviewStatus = Extract<
  UpdateState["update"]["status"],
  "available" | "checking" | "downloading" | "failed" | "idle" | "installing" | "upToDate"
>;

type OnboardingSetupPreviewState = "profile-pending" | "ready" | "required" | "unavailable";
type SettingsProfilePreviewState = "profile-pending" | "saved";

const syncPreviewStatuses = [
  { key: "synced", label: "Synced" },
  { key: "pending", label: "Pending" },
  { key: "stale", label: "Stale" },
  { key: "offline", label: "Offline" },
  { key: "authority-rejected", label: "Rejected" },
  { key: "unavailable", label: "Unavailable" },
] as const satisfies readonly { key: SyncStatus; label: string }[];

const updatePreviewStatuses = [
  { key: "idle", label: "No update" },
  { key: "available", label: "Available" },
  { key: "checking", label: "Checking" },
  { key: "downloading", label: "Downloading" },
  { key: "installing", label: "Relaunching" },
  { key: "failed", label: "Failed" },
  { key: "upToDate", label: "Complete" },
] as const satisfies readonly { key: UpdatePreviewStatus; label: string }[];

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
  syncStatus: SyncStatus;
  updateStatus: UpdatePreviewStatus;
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
  return codingProviderAccessStates.some(({ key: state }) => state === candidate)
    ? (candidate as CodingProviderAccessState)
    : fallback;
}

function resolveOnboardingStep(params: URLSearchParams): OnboardingStep {
  const candidate = params.get("onboardingStep");
  return onboardingSteps.some(({ key }) => key === candidate)
    ? (candidate as OnboardingStep)
    : "providers";
}

function resolveOnboardingSetupState(params: URLSearchParams): OnboardingSetupPreviewState {
  const candidate = params.get("setupState");
  return candidate === "profile-pending" || candidate === "required" || candidate === "unavailable"
    ? candidate
    : "ready";
}

function resolveSurface(params: URLSearchParams): DesktopSurface {
  const candidate = params.get("window");
  return candidate === "settings" || candidate === "onboarding" ? candidate : "panel";
}

function resolveSettingsProfileState(params: URLSearchParams): SettingsProfilePreviewState {
  return params.get("profileState") === "profile-pending" ? "profile-pending" : "saved";
}

function resolveSyncStatus(params: URLSearchParams): SyncStatus {
  const candidate = params.get("syncStatus");
  return syncPreviewStatuses.some(({ key }) => key === candidate)
    ? (candidate as SyncStatus)
    : "unavailable";
}

function resolveUpdateStatus(
  params: URLSearchParams,
  fixture: BrowserFixtureName,
): UpdatePreviewStatus {
  const candidate = params.get("updateStatus");
  return updatePreviewStatuses.some(({ key }) => key === candidate)
    ? (candidate as UpdatePreviewStatus)
    : fixture === "update"
      ? "available"
      : "idle";
}

function resolveBrowserFixtureName(search: string): BrowserFixtureName {
  return resolveFixture(new URLSearchParams(search));
}

function resolveDevPreviewScenario(search: string): DevPreviewScenario {
  const params = new URLSearchParams(search);
  const fixture = resolveFixture(params);
  const providerState = resolveProviderState(params, "providerState", "not-installed");
  const settingsProviderExcluded = params.get("providerState") === "excluded";

  return {
    fixture,
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
    syncStatus: resolveSyncStatus(params),
    updateStatus: resolveUpdateStatus(params, fixture),
  };
}

export {
  resolveBrowserFixtureName,
  resolveDevPreviewScenario,
  syncPreviewStatuses,
  updatePreviewStatuses,
};
export type {
  BrowserFixtureName,
  DevPreviewScenario,
  OnboardingSetupPreviewState,
  SettingsProfilePreviewState,
  UpdatePreviewStatus,
};
