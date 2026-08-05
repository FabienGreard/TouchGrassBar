import { useEffect, useState, useSyncExternalStore } from "react";

import { providerAccessStateFromPresence } from "@/components/coding-provider-access-state";
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
import { createBootstrapDelivery } from "@/native-state/bootstrap-delivery";
import { createTauriBootstrapAdapter } from "@/native-state/tauri-bootstrap-adapter";

type BootstrapDelivery = ReturnType<typeof createBootstrapDelivery>;

function OnboardingCoordinator({
  delivery: suppliedDelivery,
}: {
  delivery?: BootstrapDelivery | undefined;
}) {
  const [delivery] = useState(
    () =>
      suppliedDelivery ??
      createBootstrapDelivery(createTauriBootstrapAdapter()),
  );
  const view = useSyncExternalStore(
    delivery.subscribe,
    delivery.getSnapshot,
    delivery.getSnapshot,
  );
  const [step, setStep] = useState<OnboardingStep>("providers");
  const [furthestStep, setFurthestStep] = useState<OnboardingStep>("providers");
  const [displayName, setDisplayName] = useState("");
  const [checkingProviders, setCheckingProviders] = useState(false);
  const [submissionFailed, setSubmissionFailed] = useState(false);

  useEffect(() => {
    void delivery.read();
  }, [delivery]);

  useEffect(() => {
    const storedName = view.snapshot?.displayName;
    if (storedName && displayName.length === 0) setDisplayName(storedName);
  }, [displayName.length, view.snapshot?.displayName]);

  useEffect(() => {
    const handler = createNativeWindowKeyboardHandler({
      enabled: true,
      hide: () => void delivery.hide(),
    });
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [delivery]);

  const selectStep = (nextStep: OnboardingStep) => {
    setStep(nextStep);
    const nextIndex = onboardingSteps.findIndex(
      (candidate) => candidate.key === nextStep,
    );
    const furthestIndex = onboardingSteps.findIndex(
      (candidate) => candidate.key === furthestStep,
    );
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
    <OnboardingScreen
      busyProviders={checkingProviders}
      canComplete={
        view.phase === "ready" && view.snapshot?.persistence === "available"
      }
      codexState={providerAccessStateFromPresence(providers, "codex")}
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
      onStepChange={selectStep}
      providerState={providerAccessStateFromPresence(providers, "claude")}
      setupState={setupState}
      step={step}
      submissionState={submissionState}
    />
  );
}

export { OnboardingCoordinator };
export type { BootstrapDelivery };
