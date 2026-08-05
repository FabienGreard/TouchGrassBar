import { useEffect, useState, useSyncExternalStore } from "react";

import { providerAccessStateFromPresence } from "@/components/coding-provider-access-state";
import { createNativeWindowKeyboardHandler } from "@/components/screens/native-window-keyboard";
import { SettingsScreen } from "@/components/screens/settings/settings-screen";
import { createSettingsDelivery } from "@/native-state/settings-delivery";
import { createTauriSettingsAdapter } from "@/native-state/tauri-settings-adapter";

type SettingsDelivery = ReturnType<typeof createSettingsDelivery>;

function SettingsCoordinator({
  delivery: suppliedDelivery,
}: {
  delivery?: SettingsDelivery | undefined;
}) {
  const [delivery] = useState(
    () =>
      suppliedDelivery ?? createSettingsDelivery(createTauriSettingsAdapter()),
  );
  const view = useSyncExternalStore(
    delivery.subscribe,
    delivery.getSnapshot,
    delivery.getSnapshot,
  );
  const [checkingProviders, setCheckingProviders] = useState(false);

  useEffect(() => {
    let disposed = false;
    let stop: () => void = () => undefined;
    void delivery.activate().then((unsubscribe) => {
      if (disposed) unsubscribe();
      else stop = unsubscribe;
    });
    return () => {
      disposed = true;
      stop();
      void delivery.hideRecoveryKey();
    };
  }, [delivery]);

  useEffect(() => {
    const handler = createNativeWindowKeyboardHandler({
      enabled: true,
      hide: () => void delivery.hide(),
    });
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [delivery]);

  const state = view.snapshot;
  const launchAtLogin =
    state?.launchAtLogin.availability === "available"
      ? state.launchAtLogin.enabled
      : null;
  const providers = state?.providers;
  const profile =
    state?.profileProvisioning === "ready" &&
    typeof state.displayName === "string" &&
    typeof state.touchGrassId === "string"
      ? {
          displayName: state.displayName,
          profileKeyId: state.profileKeyId ?? null,
          recoveryKeySuffix: state.recoveryKeySuffix ?? null,
          touchGrassId: state.touchGrassId,
        }
      : null;

  return (
    <SettingsScreen
      busyProviders={checkingProviders}
      codexState={providerAccessStateFromPresence(providers, "codex")}
      launchAtLogin={launchAtLogin}
      launchAtLoginSaving={view.savingLaunchAtLogin}
      onCheckProviders={() => {
        setCheckingProviders(true);
        void delivery.read().finally(() => setCheckingProviders(false));
      }}
      onLaunchAtLoginChange={(enabled) => {
        void delivery.setLaunchAtLogin(enabled);
      }}
      onHideRecoveryKey={() => {
        void delivery.hideRecoveryKey();
      }}
      onRevealRecoveryKey={() => {
        void delivery.revealRecoveryKey();
      }}
      onSectionChange={delivery.selectSection}
      pendingDisplayName={state?.displayName}
      profile={profile}
      profileProvisioning={state?.profileProvisioning}
      providerState={providerAccessStateFromPresence(providers, "claude")}
      recoveryKey={view.recoveryKey}
      revealingRecoveryKey={view.revealingRecoveryKey}
      section={state?.section}
    />
  );
}

export { SettingsCoordinator };
export type { SettingsDelivery };
