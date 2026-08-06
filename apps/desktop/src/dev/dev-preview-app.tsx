import { useEffect, useState } from "react";

import { App } from "@/App";
import "@/dev/dev-preview.css";
import { createBrowserSanitizedDesktopStateAdapter } from "@/dev/browser-sanitized-desktop-state-adapter";
import { DevPreviewSwitcher } from "@/dev/dev-preview-switcher";
import { applyDevInstanceDocument } from "@/dev/dev-instance-document";
import { currentDevInstance } from "@/dev/dev-instance";
import { RecoverySheetPreview } from "@/dev/recovery-sheet-preview";
import {
  currentProfile,
  currentDoomerboardRows,
  currentUsagePresentation,
  myTokenmaxxerRows,
} from "@/dev/panel-fixtures";
import { resolveDevPreviewScenario } from "@/dev/preview-scenario";
import { createSanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";

document.documentElement.dataset.desktopPreview = "true";

function DevPreviewApp() {
  const [devInstance] = useState(currentDevInstance);
  const [autoUpdates, setAutoUpdates] = useState(true);
  const [launchAtLogin, setLaunchAtLogin] = useState(false);
  const [recoveryOpen, setRecoveryOpen] = useState(false);
  const [profile, setProfile] = useState({
    displayName: "Fabien",
    recoveryKeySuffix: "K9m",
    touchGrassId: "#TG-7K4P9D",
  });
  const [scenario] = useState(() =>
    resolveDevPreviewScenario(window.location.search),
  );
  const [stateDelivery] = useState(() =>
    createSanitizedDesktopStateDelivery(
      createBrowserSanitizedDesktopStateAdapter(scenario.fixture),
    ),
  );

  useEffect(() => {
    if (devInstance) {
      applyDevInstanceDocument(devInstance, scenario.surface);
    }
  }, [devInstance, scenario.surface]);
  const hasCurrentPanelPresentation =
    scenario.fixture === "current" || scenario.fixture === "update";
  const panelPresentation = hasCurrentPanelPresentation
    ? {
        currentProfile,
        doomerboardRows: currentDoomerboardRows,
        tokenmaxxerRows: myTokenmaxxerRows,
        updateAvailable: scenario.fixture === "update",
        usagePresentation: currentUsagePresentation,
      }
    : undefined;

  let surface;
  switch (scenario.surface) {
    case "onboarding":
      surface = (
        <App
          hasNativeRuntime={false}
          onboarding={{
            ...scenario.onboarding,
            initialDisplayName: "Fabien",
            onCheckProvider: () => undefined,
            onFinish: () => undefined,
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
            codexState: "ready",
            launchAtLogin,
            onAutoUpdatesChange: setAutoUpdates,
            onCheckProviders: () => undefined,
            onCheckForUpdates: () => undefined,
            onLaunchAtLoginChange: setLaunchAtLogin,
            onOpenSource: () => undefined,
            onProfileDisplayNameChange: (displayName) =>
              setProfile((current) => ({ ...current, displayName })),
            onStartRecovery: () => setRecoveryOpen(true),
            pendingDisplayName: profile.displayName,
            profile:
              scenario.settingsProfileState === "profile-pending"
                ? null
                : profile,
            profileProvisioning:
              scenario.settingsProfileState === "profile-pending"
                ? "profile-pending"
                : "ready",
            providerState: scenario.settingsProviderState,
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

  return (
    <>
      {surface}
      <RecoverySheetPreview
        onOpenChange={setRecoveryOpen}
        open={recoveryOpen}
      />
      <DevPreviewSwitcher
        activeFixture={scenario.fixture}
        activeSurface={scenario.surface}
        devInstance={devInstance}
        onboardingCodexPreviewState={
          scenario.surface === "onboarding"
            ? scenario.onboarding.codexState
            : undefined
        }
        onboardingProviderPreviewState={
          scenario.surface === "onboarding"
            ? scenario.onboarding.providerState
            : undefined
        }
        onboardingStep={
          scenario.surface === "onboarding"
            ? scenario.onboarding.initialStep
            : undefined
        }
        settingsProviderPreviewState={
          scenario.surface === "settings"
            ? scenario.settingsProviderState
            : undefined
        }
      />
    </>
  );
}

export { DevPreviewApp };
