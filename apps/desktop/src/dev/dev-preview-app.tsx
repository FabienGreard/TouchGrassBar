import { useEffect, useRef, useState } from "react";
import type { CodingProvider, UpdateState } from "@touchgrass/contracts";

import { App } from "@/App";
import "@/dev/dev-preview.css";
import { createBrowserSanitizedDesktopStateAdapter } from "@/dev/browser-sanitized-desktop-state-adapter";
import { DevPreviewSwitcher } from "@/dev/dev-preview-switcher";
import { applyDevInstanceDocument } from "@/dev/dev-instance-document";
import { currentDevInstance } from "@/dev/dev-instance";
import { RecoveryDialog } from "@/components/dialogs/recovery-dialog";
import { currentProfile, currentDoomerboardRows, myTokenmaxxerRows } from "@/dev/panel-fixtures";
import { resolveDevPreviewScenario, type UpdatePreviewStatus } from "@/dev/preview-scenario";
import { createSanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";

document.documentElement.dataset.desktopPreview = "true";

const previewCurrentVersion = "1.3.2";
const previewUpdateVersion = "1.4.0";

function previewUpdateState(
  status: UpdatePreviewStatus,
  automaticChecksEnabled: boolean,
): UpdateState {
  const base = {
    automaticChecksEnabled,
    contractVersion: 2 as const,
    currentVersion: status === "upToDate" ? previewUpdateVersion : previewCurrentVersion,
    onlineFeaturesPaused: false,
  };

  switch (status) {
    case "available":
      return { ...base, update: { status, version: previewUpdateVersion } };
    case "checking":
    case "idle":
    case "upToDate":
      return { ...base, update: { status } };
    case "downloading":
      return {
        ...base,
        update: { progressPercent: 42, status, version: previewUpdateVersion },
      };
    case "failed":
      return {
        ...base,
        update: { failure: "network", status, version: previewUpdateVersion },
      };
    case "installing":
      return { ...base, update: { status, version: previewUpdateVersion } };
  }
}

type ProviderEnablement = Record<CodingProvider, boolean>;

const defaultProviderEnablement: ProviderEnablement = {
  claude: true,
  codex: true,
};
const providerEnablementStorageKey = "touchgrass:dev-provider-enablement:v1";

function isProviderEnablement(value: unknown): value is ProviderEnablement {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Record<string, unknown>;
  return typeof candidate.claude === "boolean" && typeof candidate.codex === "boolean";
}

function readProviderEnablement(): ProviderEnablement {
  try {
    const stored = window.sessionStorage.getItem(providerEnablementStorageKey);
    if (stored === null) return { ...defaultProviderEnablement };
    const candidate: unknown = JSON.parse(stored);
    return isProviderEnablement(candidate) ? candidate : { ...defaultProviderEnablement };
  } catch {
    return { ...defaultProviderEnablement };
  }
}

function persistProviderEnablement(providerEnablement: ProviderEnablement) {
  try {
    window.sessionStorage.setItem(providerEnablementStorageKey, JSON.stringify(providerEnablement));
  } catch {
    // The development preview remains usable when storage is unavailable.
  }
}

function DevPreviewApp() {
  const surfaceContainerRef = useRef<HTMLDivElement>(null);
  const [scenario] = useState(() => resolveDevPreviewScenario(window.location.search));
  const [devInstance] = useState(currentDevInstance);
  const [autoUpdates, setAutoUpdates] = useState(true);
  const [launchAtLogin, setLaunchAtLogin] = useState(false);
  const [updateStatus, setUpdateStatus] = useState<UpdatePreviewStatus>(scenario.updateStatus);
  const [providerEnabled, setProviderEnabled] = useState<Record<CodingProvider, boolean>>(() => ({
    ...readProviderEnablement(),
    ...(scenario.surface === "settings" ? { claude: scenario.settingsProviderEnabled } : {}),
  }));
  const [recoveryOpen, setRecoveryOpen] = useState(false);
  const [profile, setProfile] = useState({
    displayName: "Fabien",
    recoveryKeySuffix: "K9m",
    touchGrassId: "#TG-7K4P9D",
  });
  const [stateDelivery] = useState(() =>
    createSanitizedDesktopStateDelivery(
      createBrowserSanitizedDesktopStateAdapter(
        scenario.fixture,
        () => new Date(),
        providerEnabled,
        scenario.syncStatus,
      ),
    ),
  );

  useEffect(() => {
    persistProviderEnablement(providerEnabled);
  }, [providerEnabled]);

  useEffect(() => {
    if (devInstance) {
      applyDevInstanceDocument(devInstance, scenario.surface);
    }
  }, [devInstance, scenario.surface]);
  const hasCurrentPanelPresentation =
    scenario.fixture === "current" || scenario.fixture === "update";
  const updateState = previewUpdateState(updateStatus, autoUpdates);
  const panelPresentation = {
    onUpdate: () => setUpdateStatus("downloading"),
    updateState,
    ...(hasCurrentPanelPresentation
      ? {
          currentProfile,
          doomerboardRows: currentDoomerboardRows,
          tokenmaxxerRows: myTokenmaxxerRows,
        }
      : {}),
  };

  let surface;
  switch (scenario.surface) {
    case "onboarding":
      surface = (
        <App
          hasNativeRuntime={false}
          onboarding={{
            appVersion: previewCurrentVersion,
            initialDisplayName: "Fabien",
            initialStep: scenario.onboarding.initialStep,
            onCheckProvider: () => undefined,
            onFinish: () => undefined,
            onStartRecovery: () => setRecoveryOpen(true),
            providers: [
              {
                displayName: "Codex",
                provider: "codex",
                state: scenario.onboarding.codexState,
              },
              {
                displayName: "Claude",
                provider: "claude",
                state: scenario.onboarding.providerState,
              },
            ],
            setupState: scenario.onboarding.setupState,
          }}
          surface="onboarding"
        />
      );
      break;
    case "settings":
      surface = (
        <App
          hasNativeRuntime={false}
          settings={{
            autoUpdates,
            launchAtLogin,
            onAutoUpdatesChange: setAutoUpdates,
            onCheckProviders: () => undefined,
            onCheckForUpdates: () => setUpdateStatus("checking"),
            onInstallUpdate: () => setUpdateStatus("downloading"),
            onLaunchAtLoginChange: setLaunchAtLogin,
            onOpenLatestDmg: () => undefined,
            onOpenSource: () => undefined,
            onProviderEnabledChange: (provider, enabled) =>
              setProviderEnabled((current) => ({
                ...current,
                [provider]: enabled,
              })),
            onProfileDisplayNameChange: (displayName) =>
              setProfile((current) => ({ ...current, displayName })),
            onStartRecovery: () => setRecoveryOpen(true),
            onRetryUpdate: () => setUpdateStatus("downloading"),
            pendingDisplayName: profile.displayName,
            profile: scenario.settingsProfileState === "profile-pending" ? null : profile,
            profileProvisioning:
              scenario.settingsProfileState === "profile-pending" ? "profile-pending" : "ready",
            providers: [
              {
                displayName: "Codex",
                enabled: providerEnabled.codex,
                provider: "codex",
                state: "detected",
              },
              {
                displayName: "Claude",
                enabled: providerEnabled.claude,
                provider: "claude",
                state: scenario.settingsProviderState,
              },
            ],
            updateState,
          }}
          surface="settings"
        />
      );
      break;
    case "panel":
      surface = (
        <App
          hasNativeRuntime={false}
          panelPresentation={panelPresentation}
          stateDelivery={stateDelivery}
          surface="panel"
        />
      );
      break;
  }

  const recoveryPortalContainer = recoveryOpen
    ? (surfaceContainerRef.current?.querySelector<HTMLElement>('[data-slot="native-window"]') ??
      null)
    : null;

  return (
    <>
      <div ref={surfaceContainerRef}>{surface}</div>
      <RecoveryDialog
        onOpenChange={setRecoveryOpen}
        onRecover={() => false}
        open={recoveryOpen}
        portalContainer={recoveryPortalContainer}
      />
      <DevPreviewSwitcher
        activeFixture={scenario.fixture}
        activeSurface={scenario.surface}
        activeSyncStatus={scenario.syncStatus}
        activeUpdateStatus={updateStatus}
        devInstance={devInstance}
        onboardingCodexPreviewState={
          scenario.surface === "onboarding" ? scenario.onboarding.codexState : undefined
        }
        onboardingProviderPreviewState={
          scenario.surface === "onboarding" ? scenario.onboarding.providerState : undefined
        }
        onboardingStep={
          scenario.surface === "onboarding" ? scenario.onboarding.initialStep : undefined
        }
        settingsProviderPreviewState={
          scenario.surface === "settings" ? scenario.settingsProviderState : undefined
        }
        settingsProviderEnabled={
          scenario.surface === "settings" ? providerEnabled.claude : undefined
        }
      />
    </>
  );
}

export { DevPreviewApp };
