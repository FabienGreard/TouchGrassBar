import { useEffect, useState, useSyncExternalStore } from "react";

import { providerAccessPresentations } from "@/components/provider-access/presentation";
import { createNativeWindowKeyboardHandler } from "@/components/screens/native-window-keyboard";
import { bindRecoveryKeyClearEvents } from "@/components/screens/settings/recovery-key-input";
import { SettingsScreen } from "@/components/screens/settings/settings-screen";
import { createSettingsDelivery } from "@/native-state/settings-delivery";
import { createTauriSettingsAdapter } from "@/native-state/tauri-settings-adapter";
import { createTauriUpdateAdapter } from "@/native-state/tauri-update-adapter";
import { createUpdateDelivery } from "@/native-state/update-delivery";

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
  const [updates] = useState(() =>
    createUpdateDelivery(createTauriUpdateAdapter()),
  );
  const view = useSyncExternalStore(
    delivery.subscribe,
    delivery.getSnapshot,
    delivery.getSnapshot,
  );
  const updateView = useSyncExternalStore(
    updates.subscribe,
    updates.getSnapshot,
    updates.getSnapshot,
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
    let disposed = false;
    let stop: () => void = () => undefined;
    void updates.activate().then((unsubscribe) => {
      if (disposed) unsubscribe();
      else stop = unsubscribe;
    });
    return () => {
      disposed = true;
      stop();
    };
  }, [updates]);

  useEffect(
    () =>
      bindRecoveryKeyClearEvents(window, () => {
        void delivery.hideRecoveryKey();
      }),
    [delivery],
  );

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
          recoveryKeySuffix: state.recoveryKeySuffix ?? null,
          touchGrassId: state.touchGrassId,
        }
      : null;

  return (
    <SettingsScreen
      busyProviders={checkingProviders}
      launchAtLogin={launchAtLogin}
      launchAtLoginSaving={view.savingLaunchAtLogin}
      onCheckProviders={() => {
        setCheckingProviders(true);
        void delivery.read().finally(() => setCheckingProviders(false));
      }}
      onCheckForUpdates={() => {
        void updates.check();
      }}
      onDeferUpdate={() => {
        void updates.defer();
      }}
      onInstallUpdate={() => {
        void updates.install();
      }}
      onLaunchAtLoginChange={(enabled) => {
        void delivery.setLaunchAtLogin(enabled);
      }}
      onOpenLatestDmg={() => {
        void updates.openLatestDmg();
      }}
      onHideRecoveryKey={() => {
        void delivery.hideRecoveryKey();
      }}
      onRevealRecoveryKey={() => {
        void delivery.revealRecoveryKey();
      }}
      onSectionChange={delivery.selectSection}
      onRetryUpdate={() => {
        void updates.retry();
      }}
      pendingDisplayName={state?.displayName}
      profile={profile}
      profileProvisioning={state?.profileProvisioning}
      providers={providerAccessPresentations(providers)}
      recoveryKey={view.recoveryKey}
      revealingRecoveryKey={view.revealingRecoveryKey}
      section={state?.section}
      updateState={updateView.state}
    />
  );
}

export { SettingsCoordinator };
export type { SettingsDelivery };
