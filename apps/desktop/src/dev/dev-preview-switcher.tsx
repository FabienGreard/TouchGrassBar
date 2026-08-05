import {
  ArrowExpand01Icon,
  ArrowShrink02Icon,
  GripVerticalIcon,
} from "@touchgrass/ui";
import { cn } from "@touchgrass/ui/lib/utils";
import {
  useEffect,
  useRef,
  useState,
  type ComponentProps,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";

import type { BrowserFixtureName } from "@/dev/preview-scenario";
import {
  codingProviderAccessStates,
  type CodingProviderAccessState,
} from "@/components/coding-provider-access-state";
import {
  onboardingSteps,
  type OnboardingStep,
} from "@/components/screens/onboarding/onboarding-flow";
import type { DevInstance } from "@/dev/dev-instance";
type PreviewSurface = "onboarding" | "panel" | "settings";

const providerPreviewStates = codingProviderAccessStates;

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
      (!position ||
        !Number.isFinite(position.left) ||
        !Number.isFinite(position.top))
    ) {
      return defaultPreviewPanelState;
    }
    return { mode: stored.mode, position };
  } catch {
    return defaultPreviewPanelState;
  }
}

function clampPreviewPanelPosition(
  left: number,
  top: number,
  width: number,
  height: number,
) {
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
  const [panelState, setPanelState] =
    useState<PreviewPanelState>(readPreviewPanelState);
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
        if (
          position.left === current.position.left &&
          position.top === current.position.top
        ) {
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
      window.sessionStorage.setItem(
        previewPanelStateStorageKey,
        JSON.stringify(panelState),
      );
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
        "backdrop-menu-glass fixed right-3 bottom-3 z-50 flex flex-col items-stretch rounded-[10px] border border-pearl-border bg-menu-glass p-1.5 text-pearl-ink shadow-menu-glass",
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
          className="flex min-w-0 flex-1 cursor-grab touch-none items-center gap-1 border-0 bg-transparent px-2 py-1 text-left font-mono text-[8px] font-semibold tracking-[0.08em] text-pearl-muted uppercase outline-none active:cursor-grabbing disabled:cursor-default focus-visible:ring-3 focus-visible:ring-ring/50"
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
      className={cn(
        "grid grid-cols-[48px_minmax(0,1fr)] items-center gap-2",
        className,
      )}
      {...props}
    >
      <small className="px-1 text-[8px] font-semibold tracking-[0.04em] text-pearl-muted uppercase">
        {label}
      </small>
      <div className="flex min-w-0 flex-wrap items-center justify-end gap-1">
        {children}
      </div>
    </div>
  );
}

type FixtureSwitcherOptionProps = {
  active: boolean;
  children: ReactNode;
  fixture: BrowserFixtureName;
};

function FixtureSwitcherOption({
  active,
  children,
  fixture,
}: FixtureSwitcherOptionProps) {
  return (
    <PreviewSwitcherOption active={active} href={`?fixture=${fixture}`}>
      {children}
    </PreviewSwitcherOption>
  );
}

type PreviewSwitcherOptionProps = {
  active: boolean;
  children: ReactNode;
  href: string;
};

function PreviewSwitcherOption({
  active,
  children,
  href,
}: PreviewSwitcherOptionProps) {
  return (
    <a
      aria-current={active ? "page" : undefined}
      className={cn(
        "rounded-[7px] px-2 py-1 text-[10px] font-semibold text-pearl-muted outline-none transition-[background-color,color,box-shadow] hover:bg-pearl-ink/5 hover:text-pearl-ink focus-visible:ring-3 focus-visible:ring-ring/50",
        active &&
          "bg-action text-accent-foreground shadow-action hover:text-accent-foreground",
      )}
      href={href}
    >
      {children}
    </a>
  );
}

function DevFixtureSwitcher({
  activeFixture,
  devInstance,
}: {
  activeFixture: BrowserFixtureName;
  devInstance?: DevInstance | null | undefined;
}) {
  return (
    <FixtureSwitcher
      aria-label="Development fixture"
      data-dev-only="preview-switcher"
      instanceLabel={devInstance?.label}
    >
      <FixtureOptions activeFixture={activeFixture} />
    </FixtureSwitcher>
  );
}

function FixtureOptions({
  activeFixture,
}: {
  activeFixture: BrowserFixtureName;
}) {
  return (
    <PreviewControlRow aria-label="Panel fixtures" label="Panel">
      <FixtureSwitcherOption
        active={activeFixture === "loading"}
        fixture="loading"
      >
        Loading
      </FixtureSwitcherOption>
      <FixtureSwitcherOption
        active={activeFixture === "unavailable"}
        fixture="unavailable"
      >
        Unavailable
      </FixtureSwitcherOption>
      <FixtureSwitcherOption
        active={activeFixture === "current"}
        fixture="current"
      >
        Current
      </FixtureSwitcherOption>
      <FixtureSwitcherOption
        active={activeFixture === "update"}
        fixture="update"
      >
        Update
      </FixtureSwitcherOption>
      <FixtureSwitcherOption active={activeFixture === "stale"} fixture="stale">
        Stale
      </FixtureSwitcherOption>
    </PreviewControlRow>
  );
}

function DevPreviewSwitcher({
  activeFixture,
  activeSurface,
  devInstance,
  onboardingCodexPreviewState,
  onboardingProviderPreviewState,
  onboardingStep,
  settingsProviderPreviewState,
}: {
  activeFixture: BrowserFixtureName;
  activeSurface: PreviewSurface;
  devInstance?: DevInstance | null | undefined;
  onboardingCodexPreviewState?: CodingProviderAccessState | undefined;
  onboardingProviderPreviewState?: CodingProviderAccessState | undefined;
  onboardingStep?: OnboardingStep | undefined;
  settingsProviderPreviewState?: CodingProviderAccessState | undefined;
}) {
  const fixture = encodeURIComponent(activeFixture);
  const settingsProviderState = settingsProviderPreviewState
    ? `&providerState=${encodeURIComponent(settingsProviderPreviewState)}`
    : "";
  const onboardingQuery =
    onboardingCodexPreviewState &&
    onboardingProviderPreviewState &&
    onboardingStep
      ? `&onboardingStep=${encodeURIComponent(onboardingStep)}&codexState=${encodeURIComponent(onboardingCodexPreviewState)}&providerState=${encodeURIComponent(onboardingProviderPreviewState)}`
      : "";
  const onboardingStateHref = ({
    codexState = onboardingCodexPreviewState ?? "ready",
    providerState = onboardingProviderPreviewState ?? "not-installed",
    step = onboardingStep ?? "providers",
  }: {
    codexState?: CodingProviderAccessState;
    providerState?: CodingProviderAccessState;
    step?: OnboardingStep;
  }) =>
    `?window=onboarding&fixture=${fixture}&onboardingStep=${encodeURIComponent(step)}&codexState=${encodeURIComponent(codexState)}&providerState=${encodeURIComponent(providerState)}`;

  return (
    <FixtureSwitcher
      data-dev-only="preview-switcher"
      instanceLabel={devInstance?.label}
    >
      <PreviewControlRow aria-label="Native surfaces" label="Surface">
        <PreviewSwitcherOption
          active={activeSurface === "panel"}
          href={`?fixture=${fixture}`}
        >
          Panel
        </PreviewSwitcherOption>
        <PreviewSwitcherOption
          active={activeSurface === "settings"}
          href={`?window=settings&fixture=${fixture}${settingsProviderState}`}
        >
          Settings
        </PreviewSwitcherOption>
        <PreviewSwitcherOption
          active={activeSurface === "onboarding"}
          href={`?window=onboarding&fixture=${fixture}${onboardingQuery}`}
        >
          Onboarding
        </PreviewSwitcherOption>
      </PreviewControlRow>
      {activeSurface === "panel" ? (
        <div className="border-t border-pearl-border/70 pt-1.5">
          <FixtureOptions activeFixture={activeFixture} />
        </div>
      ) : null}
      {activeSurface === "settings" && settingsProviderPreviewState ? (
        <PreviewControlRow
          aria-label="Claude provider preview states"
          className="border-t border-pearl-border/70 pt-1.5"
          label="Claude"
        >
          {providerPreviewStates.map((state) => (
            <PreviewSwitcherOption
              active={settingsProviderPreviewState === state.key}
              href={`?window=settings&fixture=${fixture}&providerState=${encodeURIComponent(state.key)}#settings-providers`}
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
              <PreviewControlRow
                aria-label="Codex onboarding states"
                label="Codex"
              >
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
              <PreviewControlRow
                aria-label="Claude onboarding states"
                label="Claude"
              >
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

export {
  DevFixtureSwitcher,
  DevPreviewSwitcher,
  FixtureSwitcher,
  FixtureSwitcherOption,
};
export type { PreviewSurface };
