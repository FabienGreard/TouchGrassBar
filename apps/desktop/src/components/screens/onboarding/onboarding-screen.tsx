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
import { useState } from "react";

import type { CodingProviderAccessState } from "@/components/coding-provider-access-state";
import { FinishStep } from "./finish-step";
import { onboardingSteps, type OnboardingStep } from "./onboarding-flow";
import { ProfileStep } from "./profile-step";
import { ProvidersStep } from "./providers-step";

type OnboardingScreenProps = {
  codexState?: CodingProviderAccessState;
  initialDisplayName?: string | undefined;
  initialStep?: OnboardingStep;
  onFinish?: (() => void) | undefined;
  providerState?: CodingProviderAccessState;
  setupReady?: boolean | undefined;
};

function StepBody({
  codexState,
  initialDisplayName,
  providerState,
  setupReady,
  step,
}: {
  codexState: CodingProviderAccessState;
  initialDisplayName: string;
  providerState: CodingProviderAccessState;
  setupReady: boolean;
  step: OnboardingStep;
}) {
  if (step === "providers") {
    return (
      <ProvidersStep codexState={codexState} providerState={providerState} />
    );
  }
  if (step === "profile") {
    return <ProfileStep initialDisplayName={initialDisplayName} />;
  }
  return <FinishStep setupReady={setupReady} />;
}

function StepActions({
  onFinish,
  onStepChange,
  step,
}: {
  onFinish?: (() => void) | undefined;
  onStepChange: (step: OnboardingStep) => void;
  step: OnboardingStep;
}) {
  const index = onboardingSteps.findIndex(
    (candidate) => candidate.key === step,
  );
  const previous = onboardingSteps[index - 1]?.key;
  const next = onboardingSteps[index + 1]?.key;
  const actionLabel = onboardingSteps[index]?.actionLabel;

  return (
    <div className="flex shrink-0 items-center justify-between gap-3 border-t border-sheet-line pt-4 pb-1">
      {previous ? (
        <Button
          onClick={() => onStepChange(previous)}
          type="button"
          variant="ghost"
        >
          Back
        </Button>
      ) : (
        <span />
      )}
      <Button
        disabled={!next && onFinish === undefined}
        onClick={() => {
          if (next) onStepChange(next);
          else onFinish?.();
        }}
        type="button"
      >
        {actionLabel}
      </Button>
    </div>
  );
}

function OnboardingScreen({
  codexState = "unavailable",
  initialDisplayName = "",
  initialStep = "providers",
  onFinish,
  providerState = "unavailable",
  setupReady = false,
}: OnboardingScreenProps) {
  const [step, setStep] = useState<OnboardingStep>(initialStep);
  const stepIndex = onboardingSteps.findIndex(
    (candidate) => candidate.key === step,
  );
  const detail = onboardingSteps[stepIndex] ?? onboardingSteps[0];

  return (
    <NativeWindow className="relative h-screen min-h-0 w-screen min-w-0 max-w-none min-[680px]:grid-cols-[220px_minmax(0,1fr)]">
      <NativeWindowSidebar className="h-full min-h-0 overflow-hidden px-4 py-7">
        <Brand className="px-2" />
        <NativeWindowNav aria-label="Onboarding steps" className="mt-8">
          {onboardingSteps.map((item, index) => {
            const complete = index < stepIndex;
            return (
              <NativeWindowNavItem asChild key={item.key} variant="step">
                <button
                  aria-current={step === item.key ? "step" : undefined}
                  className="cursor-pointer border-0 bg-transparent"
                  onClick={() => setStep(item.key)}
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

      <NativeWindowContent className="h-full overflow-hidden px-12 py-0">
        <div className="mx-auto flex h-full max-w-[720px] flex-col">
          <ScrollArea
            className="min-h-0 flex-1"
            viewportClassName="pt-8 pr-2 pb-3"
          >
            <small className="font-mono text-[8px] font-semibold tracking-[0.08em] text-sheet-muted uppercase">
              Step {stepIndex + 1} of 3
            </small>
            <h1 className="mt-2 mb-0 text-[30px] tracking-[-0.045em]">
              {detail.title}
            </h1>
            <p className="mt-2 mb-4 text-[11px] leading-5 text-sheet-muted">
              {detail.description}
            </p>
            <StepBody
              codexState={codexState}
              initialDisplayName={initialDisplayName}
              providerState={providerState}
              setupReady={setupReady}
              step={step}
            />
          </ScrollArea>
          <StepActions onFinish={onFinish} onStepChange={setStep} step={step} />
        </div>
      </NativeWindowContent>
    </NativeWindow>
  );
}

export { OnboardingScreen };
export type { OnboardingScreenProps };
