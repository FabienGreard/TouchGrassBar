import { cn } from "@touchgrass/ui/lib/utils";
import type { ComponentProps, ReactNode } from "react";

import type { BrowserFixtureName } from "@/browserSanitizedDesktopStateAdapter";

type FixtureSwitcherProps = ComponentProps<"aside"> & {
  children: ReactNode;
};

function FixtureSwitcher({
  children,
  className,
  ...props
}: FixtureSwitcherProps) {
  return (
    <aside
      aria-label="Development fixture"
      className={cn(
        "backdrop-menu-glass fixed right-3 bottom-3 z-50 flex items-center gap-1 rounded-[10px] border border-cream-border bg-menu-glass p-1 text-cream-ink shadow-menu-glass",
        className,
      )}
      {...props}
    >
      {children}
    </aside>
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
    <a
      aria-current={active ? "page" : undefined}
      className={cn(
        "rounded-[7px] px-2 py-1 text-[10px] font-semibold text-cream-muted transition-colors hover:bg-cream-ink/5 hover:text-cream-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-cream-focus",
        active &&
          "bg-action text-accent-foreground shadow-action hover:text-accent-foreground",
      )}
      href={`?fixture=${fixture}`}
    >
      {children}
    </a>
  );
}

function DevFixtureSwitcher({
  activeFixture,
}: {
  activeFixture: BrowserFixtureName;
}) {
  return (
    <FixtureSwitcher data-dev-only="fixture-switcher">
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
    </FixtureSwitcher>
  );
}

export { DevFixtureSwitcher, FixtureSwitcher, FixtureSwitcherOption };
