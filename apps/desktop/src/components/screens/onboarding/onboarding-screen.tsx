import {
  Brand,
  Button,
  CheckIcon,
  NativeWindow,
  NativeWindowContent,
  NativeWindowNav,
  NativeWindowNavItem,
  NativeWindowNavStepMarker,
  NativeWindowSidebar,
  ScrollArea,
} from "@touchgrass/ui";
import { useEffect, useRef, useState } from "react";

import type { CodingProviderAccessState } from "@/components/coding-provider-access-state";
import { FinishStep, type OnboardingSetupState } from "./finish-step";
import { onboardingSteps, type OnboardingStep } from "./onboarding-flow";
import { ProfileStep } from "./profile-step";
import { ProvidersStep } from "./providers-step";

type OnboardingSubmissionState = "failed" | "idle" | "submitting";

type OnboardingScreenProps = {
  busyProviders?: boolean | undefined;
  canComplete?: boolean | undefined;
  codexState?: CodingProviderAccessState;
  displayName?: string | undefined;
  furthestStep?: OnboardingStep | undefined;
  initialDisplayName?: string | undefined;
  initialStep?: OnboardingStep;
  onCheckProvider?: ((provider: "claude" | "codex") => void) | undefined;
  onDisplayNameChange?: ((displayName: string) => void) | undefined;
  onFinish?: ((displayName: string) => void) | undefined;
  onStepChange?: ((step: OnboardingStep) => void) | undefined;
  providerState?: CodingProviderAccessState;
  setupReady?: boolean | undefined;
  setupState?: OnboardingSetupState | undefined;
  step?: OnboardingStep | undefined;
  submissionState?: OnboardingSubmissionState | undefined;
};

function stepIndex(step: OnboardingStep) {
  return onboardingSteps.findIndex((candidate) => candidate.key === step);
}

function StepBody({
  busyProviders,
  codexState,
  displayName,
  onCheckProvider,
  onDisplayNameChange,
  providerState,
  setupState,
  step,
}: {
  busyProviders: boolean;
  codexState: CodingProviderAccessState;
  displayName: string;
  onCheckProvider?: ((provider: "claude" | "codex") => void) | undefined;
  onDisplayNameChange: (displayName: string) => void;
  providerState: CodingProviderAccessState;
  setupState: OnboardingSetupState;
  step: OnboardingStep;
}) {
  if (step === "providers") {
    return (
      <ProvidersStep
        busy={busyProviders}
        codexState={codexState}
        onCheckProvider={onCheckProvider}
        providerState={providerState}
      />
    );
  }
  if (step === "profile") {
    return (
      <ProfileStep
        displayName={displayName}
        onDisplayNameChange={onDisplayNameChange}
      />
    );
  }
  return <FinishStep setupState={setupState} />;
}

function StepActions({
  canComplete,
  displayName,
  onFinish,
  onStepChange,
  step,
  submissionState,
}: {
  canComplete: boolean;
  displayName: string;
  onFinish?: ((displayName: string) => void) | undefined;
  onStepChange: (step: OnboardingStep) => void;
  step: OnboardingStep;
  submissionState: OnboardingSubmissionState;
}) {
  const index = stepIndex(step);
  const previous = onboardingSteps[index - 1]?.key;
  const next = onboardingSteps[index + 1]?.key;
  const actionLabel = onboardingSteps[index]?.actionLabel;
  const validDisplayName =
    displayName.trim().length > 0 && [...displayName.trim()].length <= 40;
  const disabled = next
    ? step === "profile" && !validDisplayName
    : !canComplete ||
      !validDisplayName ||
      onFinish === undefined ||
      submissionState === "submitting";

  return (
    <div className="flex shrink-0 items-center justify-between gap-3 border-t border-sheet-line pt-4 pb-1">
      {previous ? (
        <Button
          disabled={submissionState === "submitting"}
          onClick={() => onStepChange(previous)}
          type="button"
          variant="ghost"
        >
          Back
        </Button>
      ) : (
        <span />
      )}
      <div className="flex items-center gap-3">
        <span aria-live="polite" className="text-[9px] text-sheet-muted">
          {submissionState === "failed"
            ? "Setup could not be saved. Try again."
            : submissionState === "submitting"
              ? "Saving setup…"
              : ""}
        </span>
        <Button
          disabled={disabled}
          onClick={() => {
            if (next) onStepChange(next);
            else onFinish?.(displayName.trim());
          }}
          type="button"
        >
          {submissionState === "submitting" && !next
            ? "Finishing…"
            : actionLabel}
        </Button>
      </div>
    </div>
  );
}

function OnboardingScreen({
  busyProviders = false,
  canComplete,
  codexState = "unavailable",
  displayName: controlledDisplayName,
  furthestStep: controlledFurthestStep,
  initialDisplayName = "",
  initialStep = "providers",
  onCheckProvider,
  onDisplayNameChange,
  onFinish,
  onStepChange,
  providerState = "unavailable",
  setupReady = false,
  setupState = "unavailable",
  step: controlledStep,
  submissionState = "idle",
}: OnboardingScreenProps) {
  const [localStep, setLocalStep] = useState<OnboardingStep>(initialStep);
  const [localFurthestStep, setLocalFurthestStep] =
    useState<OnboardingStep>(initialStep);
  const [localDisplayName, setLocalDisplayName] = useState(initialDisplayName);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const step = controlledStep ?? localStep;
  const furthestStep = controlledFurthestStep ?? localFurthestStep;
  const displayName = controlledDisplayName ?? localDisplayName;
  const activeStepIndex = stepIndex(step);
  const furthestStepIndex = stepIndex(furthestStep);
  const detail = onboardingSteps[activeStepIndex] ?? onboardingSteps[0];
  const resolvedSetupState = setupReady ? "ready" : setupState;
  const resolvedCanComplete = canComplete ?? onFinish !== undefined;

  useEffect(() => {
    headingRef.current?.focus({ preventScroll: true });
  }, [step]);

  const changeStep = (nextStep: OnboardingStep) => {
    const nextIndex = stepIndex(nextStep);
    if (nextIndex > furthestStepIndex + 1) return;
    if (nextIndex > furthestStepIndex && controlledFurthestStep === undefined) {
      setLocalFurthestStep(nextStep);
    }
    if (controlledStep === undefined) setLocalStep(nextStep);
    onStepChange?.(nextStep);
  };

  const changeDisplayName = (nextDisplayName: string) => {
    if (controlledDisplayName === undefined) {
      setLocalDisplayName(nextDisplayName);
    }
    onDisplayNameChange?.(nextDisplayName);
  };

  return (
    <NativeWindow className="relative h-screen min-h-0 w-screen min-w-0 max-w-none min-[680px]:grid-cols-[220px_minmax(0,1fr)]">
      <NativeWindowSidebar className="h-full min-h-0 overflow-hidden px-4 py-7">
        <Brand className="px-2" />
        <NativeWindowNav aria-label="Onboarding steps" className="mt-8">
          {onboardingSteps.map((item, index) => {
            const complete = index < activeStepIndex;
            const unavailable = index > furthestStepIndex;
            return (
              <NativeWindowNavItem asChild key={item.key} variant="step">
                <button
                  aria-current={step === item.key ? "step" : undefined}
                  className="cursor-pointer border-0 bg-transparent disabled:cursor-default disabled:opacity-45"
                  disabled={unavailable || submissionState === "submitting"}
                  onClick={() => changeStep(item.key)}
                  type="button"
                >
                  <NativeWindowNavStepMarker complete={complete}>
                    {complete ? <CheckIcon size={11} /> : index + 1}
                  </NativeWindowNavStepMarker>
                  {item.label}
                </button>
              </NativeWindowNavItem>
            );
          })}
        </NativeWindowNav>
      </NativeWindowSidebar>

      <NativeWindowContent className="h-full min-h-0 overflow-hidden p-0 min-[680px]:px-0">
        <div className="flex h-full min-h-0 flex-col">
          <ScrollArea className="min-h-0 flex-1" viewportClassName="pt-8 pb-3">
            <div className="px-12">
              <div className="mx-auto max-w-[720px]">
                <small className="font-mono text-[8px] font-semibold tracking-[0.08em] text-sheet-muted uppercase">
                  Step {activeStepIndex + 1} of 3
                </small>
                <h1
                  className="mt-2 mb-0 text-[30px] tracking-[-0.045em] outline-none"
                  ref={headingRef}
                  tabIndex={-1}
                >
                  {detail.title}
                </h1>
                <p className="mt-2 mb-4 text-[11px] leading-5 text-sheet-muted">
                  {detail.description}
                </p>
                <StepBody
                  busyProviders={busyProviders}
                  codexState={codexState}
                  displayName={displayName}
                  onCheckProvider={onCheckProvider}
                  onDisplayNameChange={changeDisplayName}
                  providerState={providerState}
                  setupState={resolvedSetupState}
                  step={step}
                />
              </div>
            </div>
          </ScrollArea>
          <div className="shrink-0 px-12">
            <div className="mx-auto max-w-[720px]">
              <StepActions
                canComplete={resolvedCanComplete}
                displayName={displayName}
                onFinish={onFinish}
                onStepChange={changeStep}
                step={step}
                submissionState={submissionState}
              />
            </div>
          </div>
        </div>
      </NativeWindowContent>
    </NativeWindow>
  );
}

export { OnboardingScreen };
export type { OnboardingScreenProps, OnboardingSubmissionState };
