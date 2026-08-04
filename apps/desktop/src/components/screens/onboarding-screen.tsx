import {
  Button,
  NativeWindow,
  NativeWindowContent,
  NativeWindowNav,
  NativeWindowNavItem,
  ProviderMark,
  ProviderStatusIcon,
} from "@touchgrass/ui";

import { ScreenSidebar } from "@/components/screens/screen-sidebar";

function ProviderStatusRow({ provider }: { provider: "claude" | "codex" }) {
  const label = provider === "codex" ? "Codex" : "Claude";

  return (
    <div className="mt-2.5 grid grid-cols-[auto_1fr_auto] items-center gap-3 rounded-[10px] border border-sheet-row-border bg-sheet-row p-3 shadow-surface">
      <span className="grid h-[29px] w-[29px] place-items-center rounded-[7px] border border-sheet-line bg-cream-highlight shadow-control">
        <ProviderMark provider={provider} />
      </span>
      <span>
        <strong className="block text-[13px]">{label}</strong>
        <small className="mt-0.5 block text-[11px] text-sheet-muted">
          Provider status unavailable
        </small>
      </span>
      <ProviderStatusIcon
        aria-label={`${label} provider status unavailable`}
        size={20}
        tone="unavailable"
      />
    </div>
  );
}

function OnboardingScreen() {
  return (
    <NativeWindow>
      <ScreenSidebar
        footer="After setup, close this window forever."
        title="Get Started"
      >
        <NativeWindowNav aria-label="Setup progress">
          <NativeWindowNavItem aria-current="page">
            1&nbsp; Providers
          </NativeWindowNavItem>
          <NativeWindowNavItem aria-disabled="true">
            2&nbsp; Identity
          </NativeWindowNavItem>
          <NativeWindowNavItem aria-disabled="true">
            3&nbsp; Recovery
          </NativeWindowNavItem>
        </NativeWindowNav>
      </ScreenSidebar>

      <NativeWindowContent>
        <div className="mx-auto flex w-full max-w-[620px] flex-col justify-center">
          <small className="font-mono text-[10px] font-semibold tracking-[0.12em] text-sheet-green uppercase">
          TouchGrassBar setup
          </small>
          <h1 className="mt-2.5 mb-3 text-[36px] leading-none tracking-[-0.045em]">
            Confirm your providers.
          </h1>
          <p className="mt-0 mb-5 text-[14px] leading-6 text-sheet-muted">
            TouchGrassBar will check the Codex and Claude CLIs already installed
            on this Mac.
          </p>

        <ProviderStatusRow provider="codex" />
        <ProviderStatusRow provider="claude" />

          <div className="mt-6">
            <Button disabled size="sheet" type="button">
              Continue
            </Button>
          </div>
          <small className="mt-4 text-[10px] text-sheet-muted">
            Prompts and conversations never leave this Mac.
          </small>
        </div>
      </NativeWindowContent>
    </NativeWindow>
  );
}

export { OnboardingScreen };
