import { CheckIcon, MacMenuBarPreview } from "@touchgrass/ui";

type OnboardingSetupState =
  "profile-pending" | "ready" | "required" | "unavailable";

function setupCopy(state: OnboardingSetupState) {
  if (state === "ready") {
    return {
      detail: "Your public Profile and local setup are ready.",
      title: "Local setup ready",
    };
  }
  if (state === "profile-pending") {
    return {
      detail:
        "Creation retries automatically while local provider utility stays available.",
      title: "Profile Pending",
    };
  }
  if (state === "required") {
    return {
      detail:
        "If Profile services are unavailable, setup completes as Profile Pending, retries automatically, and local provider utility stays available.",
      title: "Ready to finish setup",
    };
  }
  return {
    detail:
      "Local setup storage is unavailable. Profile Pending cannot be recorded safely yet; local provider utility stays available.",
    title: "Setup is not connected yet",
  };
}

function FinishStep({ setupState }: { setupState: OnboardingSetupState }) {
  const copy = setupCopy(setupState);
  const locallyComplete = setupState !== "unavailable";
  return (
    <div className="grid gap-3">
      <div
        className="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-3 rounded-[12px] border border-sheet-line bg-white/38 p-4 shadow-surface"
        aria-live="polite"
        data-setup-state={setupState}
      >
        <span className="grid size-8 shrink-0 place-items-center rounded-full bg-action text-accent-foreground">
          {locallyComplete ? (
            <CheckIcon size={15} />
          ) : (
            <span aria-hidden="true">—</span>
          )}
        </span>
        <span className="min-w-0">
          <strong className="block text-[12px]">{copy.title}</strong>
          <small className="mt-1 block text-[9px] leading-4 text-sheet-muted">
            {copy.detail}
          </small>
        </span>
      </div>
      <div className="grid gap-2 px-1 pt-1">
        <span>
          <strong className="block text-[12px]">
            Open TouchGrassBar anytime
          </strong>
          <small className="mt-1 block text-[9px] leading-4 text-sheet-muted">
            Click the highlighted menu bar icon whenever you want to check your
            limits.
          </small>
        </span>
        <MacMenuBarPreview />
      </div>
    </div>
  );
}

export { FinishStep };
export type { OnboardingSetupState };
