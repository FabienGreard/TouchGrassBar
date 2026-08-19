import { useEffect, useState, useSyncExternalStore } from "react";

import { providerAccessPresentations } from "@/components/provider-access/presentation";
import { createNativeWindowKeyboardHandler } from "@/components/screens/native-window-keyboard";
import {
  OnboardingScreen,
  type OnboardingSubmissionState,
} from "@/components/screens/onboarding/onboarding-screen";
import type { OnboardingSetupState } from "@/components/screens/onboarding/finish-step";
import {
  onboardingSteps,
  type OnboardingStep,
} from "@/components/screens/onboarding/onboarding-flow";
import { RecoveryDialog } from "@/components/dialogs/recovery-dialog";
import { createBootstrapDelivery } from "@/native-state/bootstrap-delivery";
import { createTauriBootstrapAdapter } from "@/native-state/tauri-bootstrap-adapter";
import { createTauriUpdateAdapter } from "@/native-state/tauri-update-adapter";
import { createUpdateDelivery } from "@/native-state/update-delivery";

type BootstrapDelivery = ReturnType<typeof createBootstrapDelivery>;

function OnboardingCoordinator({
  delivery: suppliedDelivery,
}: {
  delivery?: BootstrapDelivery | undefined;
}) {
  const [delivery] = useState(
    () => suppliedDelivery ?? createBootstrapDelivery(createTauriBootstrapAdapter()),
  );
  const [updates] = useState(() => createUpdateDelivery(createTauriUpdateAdapter()));
  const view = useSyncExternalStore(delivery.subscribe, delivery.getSnapshot, delivery.getSnapshot);
  const updateView = useSyncExternalStore(
    updates.subscribe,
    updates.getSnapshot,
    updates.getSnapshot,
  );
  const [step, setStep] = useState<OnboardingStep>("providers");
  const [furthestStep, setFurthestStep] = useState<OnboardingStep>("providers");
  const [displayName, setDisplayName] = useState("");
  const [checkingProviders, setCheckingProviders] = useState(false);
  const [submissionFailed, setSubmissionFailed] = useState(false);
  const [recoveryOpen, setRecoveryOpen] = useState(false);

  useEffect(() => {
    void delivery.read();
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

  useEffect(() => {
    const storedName = view.snapshot?.displayName;
    if (storedName && displayName.length === 0) setDisplayName(storedName);
  }, [displayName.length, view.snapshot?.displayName]);

  useEffect(() => {
    const handler = createNativeWindowKeyboardHandler({
      enabled: !recoveryOpen,
      hide: () => void delivery.hide(),
    });
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [delivery, recoveryOpen]);

  const selectStep = (nextStep: OnboardingStep) => {
    setStep(nextStep);
    const nextIndex = onboardingSteps.findIndex((candidate) => candidate.key === nextStep);
    const furthestIndex = onboardingSteps.findIndex((candidate) => candidate.key === furthestStep);
    if (nextIndex > furthestIndex) setFurthestStep(nextStep);
  };

  const providers = view.snapshot?.providers;
  const setupState: OnboardingSetupState =
    view.snapshot?.persistence !== "available"
      ? "unavailable"
      : view.snapshot.profileProvisioning === "profile-pending"
        ? "profile-pending"
        : view.snapshot.profileProvisioning === "ready"
          ? "ready"
          : "required";
  const submissionState: OnboardingSubmissionState = view.submitting
    ? "submitting"
    : submissionFailed
      ? "failed"
      : "idle";

  return (
    <>
      <OnboardingScreen
      appVersion={updateView.state?.currentVersion}
      busyProviders={checkingProviders}
      canComplete={view.phase === "ready" && view.snapshot?.persistence === "available"}
      displayName={displayName}
      furthestStep={furthestStep}
      onCheckProvider={() => {
        setCheckingProviders(true);
        void delivery.read().finally(() => setCheckingProviders(false));
      }}
      onDisplayNameChange={(nextDisplayName) => {
        setSubmissionFailed(false);
        setDisplayName(nextDisplayName);
      }}
      onFinish={(nextDisplayName) => {
        setSubmissionFailed(false);
        void delivery.complete(nextDisplayName).then((completed) => {
          setSubmissionFailed(!completed);
        });
      }}
      onStartRecovery={() => {
        setSubmissionFailed(false);
        setRecoveryOpen(true);
      }}
      onStepChange={selectStep}
      providers={providerAccessPresentations(providers)}
      setupState={setupState}
      step={step}
      submissionState={submissionState}
      />
      <RecoveryDialog
        onOpenChange={setRecoveryOpen}
        onRecover={(credentials) => delivery.recoverProfile(credentials)}
        open={recoveryOpen}
      />
    </>
  );
}

export { OnboardingCoordinator };
export type { BootstrapDelivery };
