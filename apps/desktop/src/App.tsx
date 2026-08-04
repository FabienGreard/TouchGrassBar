import {
  PanelScreen,
  type PanelPresentation,
} from "@/components/panel/panel-screen";
import {
  OnboardingScreen,
  type OnboardingScreenProps,
} from "@/components/screens/onboarding/onboarding-screen";
import {
  SettingsScreen,
  type SettingsScreenProps,
} from "@/components/screens/settings/settings-screen";
import type { SanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";

type DesktopSurface = "onboarding" | "panel" | "settings";

type AppProps =
  | {
      hasNativeRuntime: boolean;
      onboarding?: OnboardingScreenProps | undefined;
      surface: "onboarding";
    }
  | {
      hasNativeRuntime: boolean;
      panelPresentation?: PanelPresentation | undefined;
      stateDelivery: SanitizedDesktopStateDelivery;
      surface: "panel";
    }
  | {
      hasNativeRuntime: boolean;
      settings?: SettingsScreenProps | undefined;
      surface: "settings";
    };

function App(props: AppProps) {
  switch (props.surface) {
    case "settings":
      return <SettingsScreen {...props.settings} />;
    case "onboarding":
      return <OnboardingScreen {...props.onboarding} />;
    case "panel":
      return (
        <PanelScreen
          hasNativeRuntime={props.hasNativeRuntime}
          presentation={props.panelPresentation}
          stateDelivery={props.stateDelivery}
        />
      );
  }
}

export { App };
export type { AppProps, DesktopSurface };
