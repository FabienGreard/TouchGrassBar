import { CheckIcon, MacMenuBarPreview } from "@touchgrass/ui";

function FinishStep({ setupReady = false }: { setupReady?: boolean }) {
  return (
    <div className="grid gap-3">
      <div
        className="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-3 rounded-[12px] border border-sheet-line bg-white/38 p-4 shadow-surface"
        data-setup-state={setupReady ? "ready" : "unavailable"}
      >
        <span className="grid size-8 shrink-0 place-items-center rounded-full bg-action text-accent-foreground">
          {setupReady ? (
            <CheckIcon size={15} />
          ) : (
            <span aria-hidden="true">—</span>
          )}
        </span>
        <span className="min-w-0">
          <strong className="block text-[12px]">
            {setupReady ? "Local setup ready" : "Setup is not connected yet"}
          </strong>
          <small className="mt-1 block text-[9px] leading-4 text-sheet-muted">
            Profile creation and recovery are not connected in this build.
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
