import { getCurrentWindow } from "@tauri-apps/api/window";

import type { BrowserFixtureName } from "@/browserSanitizedDesktopStateAdapter";
import { DevFixtureSwitcher } from "@/components/dev-fixture-switcher";
import { PanelScreen } from "@/components/panel/panel-screen";
import { OnboardingScreen } from "@/components/screens/onboarding-screen";
import { SettingsScreen } from "@/components/screens/settings-screen";
import type { SanitizedDesktopStateDelivery } from "@/sanitizedDesktopStateDelivery";

type AppProps = {
  hasNativeRuntime: boolean;
  previewFixtureName: BrowserFixtureName;
  stateDelivery: SanitizedDesktopStateDelivery;
};

export function App({
  hasNativeRuntime,
  previewFixtureName,
  stateDelivery,
}: AppProps) {
  const hasBrowserPreview = import.meta.env.DEV && !hasNativeRuntime;
  const previewLabel = hasBrowserPreview
    ? new URLSearchParams(window.location.search).get("window")
    : null;
  const label = hasNativeRuntime
    ? getCurrentWindow().label
    : (previewLabel ?? "panel");

  switch (label) {
    case "settings":
      return <SettingsScreen />;
    case "onboarding":
      return <OnboardingScreen />;
    default:
      return (
        <>
          <PanelScreen
            hasNativeRuntime={hasNativeRuntime}
            previewFixtureName={previewFixtureName}
            stateDelivery={stateDelivery}
          />
          {hasBrowserPreview ? (
            <DevFixtureSwitcher activeFixture={previewFixtureName} />
          ) : null}
        </>
      );
  }
}
