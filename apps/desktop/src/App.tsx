import { getCurrentWindow } from "@tauri-apps/api/window";

import { DevFixtureSwitcher } from "./components/dev-fixture-switcher";
import { PanelScreen } from "./components/panel/panel-screen";
import { OnboardingScreen } from "./components/screens/onboarding-screen";
import { SettingsScreen } from "./components/screens/settings-screen";
import { resolveBrowserFixtureName } from "./nativeState";

export function App() {
  const hasNativeRuntime = "__TAURI_INTERNALS__" in window;
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
          <PanelScreen />
          {hasBrowserPreview ? (
            <DevFixtureSwitcher
              activeFixture={resolveBrowserFixtureName(window.location.search)}
            />
          ) : null}
        </>
      );
  }
}
