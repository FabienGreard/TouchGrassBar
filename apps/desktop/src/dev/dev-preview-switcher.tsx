import type { SyncStatus } from "@touchgrass/contracts";
import { ArrowExpand01Icon, ArrowShrink02Icon, GripVerticalIcon } from "@touchgrass/ui";
import { cn } from "@touchgrass/ui/lib/utils";
import {
  useEffect,
  useRef,
  useState,
  type ComponentProps,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";

import { syncPreviewStatuses, type BrowserFixtureName } from "@/dev/preview-scenario";
import {
  codingProviderAccessStates,
  type CodingProviderAccessState,
} from "@/components/provider-access/presentation";
import {
  onboardingSteps,
  type OnboardingStep,
} from "@/components/screens/onboarding/onboarding-flow";
import type { DevInstance } from "@/dev/dev-instance";

type PreviewSurface = "onboarding" | "panel" | "settings";

const providerPreviewStates = codingProviderAccessStates;
const settingsProviderPreviewStates = [
  ...codingProviderAccessStates,
  { key: "excluded", label: "Excluded" },
] as const;

type PreviewPanelMode = "expanded" | "minimized";
type PreviewPanelPosition = { left: number; top: number } | null;
type PreviewPanelState = {
  mode: PreviewPanelMode;
  position: PreviewPanelPosition;
};

const defaultPreviewPanelState: PreviewPanelState = {
  mode: "minimized",
  position: null,
};
const previewPanelStateStorageKey = "touchgrass:dev-preview-panel-state";

function readPreviewPanelState(): PreviewPanelState {
  if (typeof window === "undefined") return defaultPreviewPanelState;

  try {
    const stored = JSON.parse(
      window.sessionStorage.getItem(previewPanelStateStorageKey) ?? "null",
    ) as Partial<PreviewPanelState> | null;
    if (!stored || (stored.mode !== "expanded" && stored.mode !== "minimized")) {
      return defaultPreviewPanelState;
    }
    const position = stored.position;
    if (
      position !== null &&
      (!position || !Number.isFinite(position.left) || !Number.isFinite(position.top))
    ) {
      return defaultPreviewPanelState;
    }
    return { mode: stored.mode, position };
  } catch {
    return defaultPreviewPanelState;
  }
}

function clampPreviewPanelPosition(left: number, top: number, width: number, height: number) {
  const edge = 8;
  const maximumLeft = Math.max(edge, window.innerWidth - width - edge);
  const maximumTop = Math.max(edge, window.innerHeight - height - edge);
  return {
    left: Math.min(Math.max(edge, left), maximumLeft),
    top: Math.min(Math.max(edge, top), maximumTop),
  };
}

type FixtureSwitcherProps = ComponentProps<"aside"> & {
  children: ReactNode;
  instanceLabel?: string | undefined;
};

function FixtureSwitcher({
  children,
  className,
  instanceLabel,
  style,
  ...props
}: FixtureSwitcherProps) {
  const [panelState, setPanelState] = useState<PreviewPanelState>(readPreviewPanelState);
  const panelRef = useRef<HTMLElement>(null);
  const dragRef = useRef<{
    height: number;
    offsetX: number;
    offsetY: number;
    pointerId: number;
    width: number;
  } | null>(null);

  useEffect(() => {
    function keepPanelInsideViewport() {
      const bounds = panelRef.current?.getBoundingClientRect();
      if (!bounds) return;
      setPanelState((current) => {
        if (!current.position) return current;
        const position = clampPreviewPanelPosition(
          current.position.left,
          current.position.top,
          bounds.width,
          bounds.height,
        );
        if (position.left === current.position.left && position.top === current.position.top) {
          return current;
        }
        return { ...current, position };
      });
    }

    const frame = window.requestAnimationFrame(keepPanelInsideViewport);
    window.addEventListener("resize", keepPanelInsideViewport);
    return () => {
      window.cancelAnimationFrame(frame);
      window.removeEventListener("resize", keepPanelInsideViewport);
    };
  }, [panelState.mode]);

  useEffect(() => {
    try {
      window.sessionStorage.setItem(previewPanelStateStorageKey, JSON.stringify(panelState));
    } catch {
      // The development control remains usable when storage is unavailable.
    }
  }, [panelState]);

  function startDragging(event: ReactPointerEvent<HTMLButtonElement>) {
    const bounds = panelRef.current?.getBoundingClientRect();
    if (!bounds) return;

    dragRef.current = {
      height: bounds.height,
      offsetX: event.clientX - bounds.left,
      offsetY: event.clientY - bounds.top,
      pointerId: event.pointerId,
      width: bounds.width,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  }

  function dragPanel(event: ReactPointerEvent<HTMLButtonElement>) {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const position = clampPreviewPanelPosition(
      event.clientX - drag.offsetX,
      event.clientY - drag.offsetY,
      drag.width,
      drag.height,
    );
    setPanelState((current) => ({ ...current, position }));
  }

  function stopDragging(event: ReactPointerEvent<HTMLButtonElement>) {
    if (dragRef.current?.pointerId !== event.pointerId) return;
    dragRef.current = null;
    event.currentTarget.releasePointerCapture(event.pointerId);
  }

  const positionStyle =
    panelState.mode === "expanded" && panelState.position
      ? {
          bottom: "auto",
          left: panelState.position.left,
          right: "auto",
          top: panelState.position.top,
        }
      : undefined;

  return (
    <aside
      aria-label="Development preview"
      className={cn(
        "fixed right-3 bottom-3 z-50 flex flex-col items-stretch rounded-[10px] border border-pearl-border bg-menu-glass p-1.5 text-pearl-ink shadow-menu-glass backdrop-menu-glass",
        panelState.mode === "expanded" ? "min-w-[306px] gap-1.5" : "w-[142px]",
        className,
      )}
      data-slot="dev-preview-switcher"
      data-state={panelState.mode}
      ref={panelRef}
      style={{ ...positionStyle, ...style }}
      {...props}
    >
      <div className="flex items-center gap-1">
        <button
          aria-label="Drag development preview"
          className="flex min-w-0 flex-1 cursor-grab touch-none items-center gap-1 border-0 bg-transparent px-2 py-1 text-left font-mono text-[8px] font-semibold tracking-[0.08em] text-pearl-muted uppercase outline-none focus-visible:ring-3 focus-visible:ring-ring/50 active:cursor-grabbing disabled:cursor-default"
          disabled={panelState.mode === "minimized"}
          onPointerCancel={stopDragging}
          onPointerDown={startDragging}
          onPointerMove={dragPanel}
          onPointerUp={stopDragging}
          type="button"
        >
          {panelState.mode === "expanded" ? (
            <GripVerticalIcon
              aria-hidden
              className="shrink-0"
              data-icon-source="GripVerticalIcon"
              size={12}
              tone="muted"
            />
          ) : null}
          <span className="truncate">
            {instanceLabel ? `Preview · ${instanceLabel}` : "Preview state"}
          </span>
        </button>
        <button
          aria-label={
            panelState.mode === "expanded"
              ? "Minimize development preview"
              : "Expand development preview"
          }
          className="grid size-6 cursor-pointer place-items-center rounded-[6px] border-0 bg-transparent text-pearl-muted outline-none hover:bg-pearl-ink/5 hover:text-pearl-ink focus-visible:ring-3 focus-visible:ring-ring/50"
          onClick={() =>
            setPanelState((current) => ({
              ...current,
              mode: current.mode === "expanded" ? "minimized" : "expanded",
            }))
          }
          type="button"
        >
          {panelState.mode === "expanded" ? (
            <ArrowShrink02Icon data-icon-source="ArrowShrink02Icon" size={13} />
          ) : (
            <ArrowExpand01Icon data-icon-source="ArrowExpand01Icon" size={13} />
          )}
        </button>
      </div>
      <div
        className="flex flex-col gap-1.5"
        data-slot="dev-preview-controls"
        hidden={panelState.mode === "minimized"}
      >
        {children}
      </div>
    </aside>
  );
}

function PreviewControlRow({
  children,
  className,
  label,
  ...props
}: ComponentProps<"div"> & { label: string }) {
  return (
    <div
      className={cn("grid grid-cols-[48px_minmax(0,1fr)] items-center gap-2", className)}
      {...props}
    >
      <small className="px-1 text-[8px] font-semibold tracking-[0.04em] text-pearl-muted uppercase">
        {label}
      </small>
      <div className="flex min-w-0 flex-wrap items-center justify-end gap-1">{children}</div>
    </div>
  );
}

type FixtureSwitcherOptionProps = {
  active: boolean;
  children: ReactNode;
  fixture: BrowserFixtureName;
  syncStatus?: SyncStatus | undefined;
};

function FixtureSwitcherOption({
  active,
  children,
  fixture,
  syncStatus,
}: FixtureSwitcherOptionProps) {
  const syncQuery = syncStatus ? `&syncStatus=${encodeURIComponent(syncStatus)}` : "";
  return (
    <PreviewSwitcherOption active={active} href={`?fixture=${fixture}${syncQuery}`}>
      {children}
    </PreviewSwitcherOption>
  );
}

type PreviewSwitcherOptionProps = {
  active: boolean;
  children: ReactNode;
  href: string;
};

function PreviewSwitcherOption({ active, children, href }: PreviewSwitcherOptionProps) {
  return (
    <a
      aria-current={active ? "page" : undefined}
      className={cn(
        "rounded-[7px] px-2 py-1 text-[10px] font-semibold text-pearl-muted transition-[background-color,color,box-shadow] outline-none hover:bg-pearl-ink/5 hover:text-pearl-ink focus-visible:ring-3 focus-visible:ring-ring/50",
        active && "bg-action text-accent-foreground shadow-action hover:text-accent-foreground",
      )}
      href={href}
    >
      {children}
    </a>
  );
}

function DevFixtureSwitcher({
  activeFixture,
  activeSyncStatus,
  devInstance,
}: {
  activeFixture: BrowserFixtureName;
  activeSyncStatus?: SyncStatus | undefined;
  devInstance?: DevInstance | null | undefined;
}) {
  return (
    <FixtureSwitcher
      aria-label="Development fixture"
      data-dev-only="preview-switcher"
      instanceLabel={devInstance?.label}
    >
      <FixtureOptions activeFixture={activeFixture} activeSyncStatus={activeSyncStatus} />
    </FixtureSwitcher>
  );
}

function FixtureOptions({
  activeFixture,
  activeSyncStatus,
}: {
  activeFixture: BrowserFixtureName;
  activeSyncStatus?: SyncStatus | undefined;
}) {
  return (
    <PreviewControlRow aria-label="Panel fixtures" label="Panel">
      <FixtureSwitcherOption
        active={activeFixture === "loading"}
        fixture="loading"
        syncStatus={activeSyncStatus}
      >
        Loading
      </FixtureSwitcherOption>
      <FixtureSwitcherOption
        active={activeFixture === "unavailable"}
        fixture="unavailable"
        syncStatus={activeSyncStatus}
      >
        Unavailable
      </FixtureSwitcherOption>
      <FixtureSwitcherOption
        active={activeFixture === "current"}
        fixture="current"
        syncStatus={activeSyncStatus}
      >
        Current
      </FixtureSwitcherOption>
      <FixtureSwitcherOption
        active={activeFixture === "update"}
        fixture="update"
        syncStatus={activeSyncStatus}
      >
        Update
      </FixtureSwitcherOption>
      <FixtureSwitcherOption
        active={activeFixture === "stale"}
        fixture="stale"
        syncStatus={activeSyncStatus}
      >
        Stale
      </FixtureSwitcherOption>
    </PreviewControlRow>
  );
}

function DevPreviewSwitcher({
  activeFixture,
  activeSurface,
  activeSyncStatus,
  devInstance,
  onboardingCodexPreviewState,
  onboardingProviderPreviewState,
  onboardingStep,
  settingsProviderEnabled = true,
  settingsProviderPreviewState,
}: {
  activeFixture: BrowserFixtureName;
  activeSurface: PreviewSurface;
  activeSyncStatus: SyncStatus;
  devInstance?: DevInstance | null | undefined;
  onboardingCodexPreviewState?: CodingProviderAccessState | undefined;
  onboardingProviderPreviewState?: CodingProviderAccessState | undefined;
  onboardingStep?: OnboardingStep | undefined;
  settingsProviderEnabled?: boolean | undefined;
  settingsProviderPreviewState?: CodingProviderAccessState | undefined;
}) {
  const fixture = encodeURIComponent(activeFixture);
  const syncStatus = encodeURIComponent(activeSyncStatus);
  const settingsProviderState = settingsProviderPreviewState
    ? `&providerState=${encodeURIComponent(
        settingsProviderEnabled ? settingsProviderPreviewState : "excluded",
      )}`
    : "";
  const onboardingQuery =
    onboardingCodexPreviewState && onboardingProviderPreviewState && onboardingStep
      ? `&onboardingStep=${encodeURIComponent(onboardingStep)}&codexState=${encodeURIComponent(onboardingCodexPreviewState)}&providerState=${encodeURIComponent(onboardingProviderPreviewState)}`
      : "";
  const onboardingStateHref = ({
    codexState = onboardingCodexPreviewState ?? "detected",
    providerState = onboardingProviderPreviewState ?? "not-installed",
    step = onboardingStep ?? "providers",
  }: {
    codexState?: CodingProviderAccessState;
    providerState?: CodingProviderAccessState;
    step?: OnboardingStep;
  }) =>
    `?window=onboarding&fixture=${fixture}&onboardingStep=${encodeURIComponent(step)}&codexState=${encodeURIComponent(codexState)}&providerState=${encodeURIComponent(providerState)}&syncStatus=${syncStatus}`;

  return (
    <FixtureSwitcher data-dev-only="preview-switcher" instanceLabel={devInstance?.label}>
      <PreviewControlRow aria-label="Native surfaces" label="Surface">
        <PreviewSwitcherOption
          active={activeSurface === "panel"}
          href={`?fixture=${fixture}&syncStatus=${syncStatus}`}
        >
          Panel
        </PreviewSwitcherOption>
        <PreviewSwitcherOption
          active={activeSurface === "settings"}
          href={`?window=settings&fixture=${fixture}${settingsProviderState}&syncStatus=${syncStatus}`}
        >
          Settings
        </PreviewSwitcherOption>
        <PreviewSwitcherOption
          active={activeSurface === "onboarding"}
          href={`?window=onboarding&fixture=${fixture}${onboardingQuery}&syncStatus=${syncStatus}`}
        >
          Onboarding
        </PreviewSwitcherOption>
      </PreviewControlRow>
      {activeSurface === "panel" ? (
        <div className="border-t border-pearl-border/70 pt-1.5">
          <FixtureOptions activeFixture={activeFixture} activeSyncStatus={activeSyncStatus} />
          <PreviewControlRow aria-label="Sync preview states" label="Sync">
            {syncPreviewStatuses.map((status) => (
              <PreviewSwitcherOption
                active={activeSyncStatus === status.key}
                href={`?fixture=${fixture}&syncStatus=${encodeURIComponent(status.key)}`}
                key={status.key}
              >
                {status.label}
              </PreviewSwitcherOption>
            ))}
          </PreviewControlRow>
        </div>
      ) : null}
      {activeSurface === "settings" ? (
        <PreviewControlRow
          aria-label="Update preview states"
          className="border-t border-pearl-border/70 pt-1.5"
          label="Update"
        >
          <PreviewSwitcherOption
            active={activeFixture !== "update"}
            href={`?window=settings&fixture=current${settingsProviderState}&syncStatus=${syncStatus}#settings-general`}
          >
            No update
          </PreviewSwitcherOption>
          <PreviewSwitcherOption
            active={activeFixture === "update"}
            href={`?window=settings&fixture=update${settingsProviderState}&syncStatus=${syncStatus}#settings-general`}
          >
            Available
          </PreviewSwitcherOption>
        </PreviewControlRow>
      ) : null}
      {activeSurface === "settings" && settingsProviderPreviewState ? (
        <PreviewControlRow aria-label="Claude provider preview states" label="Claude">
          {settingsProviderPreviewStates.map((state) => (
            <PreviewSwitcherOption
              active={
                state.key === "excluded"
                  ? !settingsProviderEnabled
                  : settingsProviderEnabled && settingsProviderPreviewState === state.key
              }
              href={`?window=settings&fixture=${fixture}&providerState=${encodeURIComponent(state.key)}&syncStatus=${syncStatus}#settings-providers`}
              key={state.key}
            >
              {state.label}
            </PreviewSwitcherOption>
          ))}
        </PreviewControlRow>
      ) : null}
      {activeSurface === "onboarding" &&
      onboardingCodexPreviewState &&
      onboardingProviderPreviewState &&
      onboardingStep ? (
        <>
          <PreviewControlRow
            aria-label="Onboarding steps"
            className="border-t border-pearl-border/70 pt-1.5"
            label="Step"
          >
            {onboardingSteps.map((step) => (
              <PreviewSwitcherOption
                active={onboardingStep === step.key}
                href={onboardingStateHref({ step: step.key })}
                key={step.key}
              >
                {step.label}
              </PreviewSwitcherOption>
            ))}
          </PreviewControlRow>
          {onboardingStep === "providers" ? (
            <>
              <PreviewControlRow aria-label="Codex onboarding states" label="Codex">
                {providerPreviewStates.map((state) => (
                  <PreviewSwitcherOption
                    active={onboardingCodexPreviewState === state.key}
                    href={onboardingStateHref({ codexState: state.key })}
                    key={state.key}
                  >
                    {state.key === "not-installed" ? "Missing" : state.label}
                  </PreviewSwitcherOption>
                ))}
              </PreviewControlRow>
              <PreviewControlRow aria-label="Claude onboarding states" label="Claude">
                {providerPreviewStates.map((state) => (
                  <PreviewSwitcherOption
                    active={onboardingProviderPreviewState === state.key}
                    href={onboardingStateHref({ providerState: state.key })}
                    key={state.key}
                  >
                    {state.key === "not-installed" ? "Missing" : state.label}
                  </PreviewSwitcherOption>
                ))}
              </PreviewControlRow>
            </>
          ) : null}
        </>
      ) : null}
    </FixtureSwitcher>
  );
}

export { DevFixtureSwitcher, DevPreviewSwitcher, FixtureSwitcher, FixtureSwitcherOption };
export type { PreviewSurface };
