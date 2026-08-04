import {
  PanelScreen,
  type PanelPresentation,
} from "@/components/panel/panel-screen";
import {
  OnboardingScreen,
  type OnboardingScreenProps,
} from "@/components/screens/onboarding/onboarding-screen";
import { OnboardingCoordinator } from "@/components/screens/onboarding/onboarding-coordinator";
import {
  SettingsScreen,
  type SettingsScreenProps,
} from "@/components/screens/settings/settings-screen";
import { SettingsCoordinator } from "@/components/screens/settings/settings-coordinator";
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
      return props.hasNativeRuntime && props.settings === undefined ? (
        <SettingsCoordinator />
      ) : (
        <SettingsScreen {...props.settings} />
      );
    case "onboarding":
      return props.hasNativeRuntime && props.onboarding === undefined ? (
        <OnboardingCoordinator />
      ) : (
        <OnboardingScreen {...props.onboarding} />
      );
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
