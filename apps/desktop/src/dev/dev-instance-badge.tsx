import type { CSSProperties } from "react";

import type { DesktopSurface } from "@/App";
import { devAccentColors, type DevInstance } from "@/dev/dev-instance";
import "@/dev/dev-instance.css";

const accentClasses = {
  amber: "bg-amber-500 ring-amber-500/25",
  blue: "bg-blue-500 ring-blue-500/25",
  rose: "bg-rose-500 ring-rose-500/25",
  teal: "bg-teal-500 ring-teal-500/25",
  violet: "bg-violet-500 ring-violet-500/25",
} as const;

function DevInstanceBadge({
  instance,
  surface,
}: {
  instance: DevInstance;
  surface: DesktopSurface;
}) {
  const position =
    surface === "panel"
      ? "top-3 right-[52px] max-w-[138px]"
      : "top-3 right-3 max-w-[180px]";

  return (
    <aside
      aria-label={`Development instance ${instance.label}`}
      className={`backdrop-menu-glass fixed z-[49] flex h-7 items-center gap-1.5 rounded-full border border-pearl-border bg-menu-glass px-2 text-pearl-ink shadow-menu-glass ${position}`}
      data-dev-instance-accent={instance.accent}
      data-dev-instance-surface={surface}
      data-slot="dev-instance-badge"
      style={
        {
          "--dev-instance-accent": devAccentColors[instance.accent],
        } as CSSProperties
      }
      title={instance.label}
    >
      <span
        aria-hidden="true"
        className={`size-2 shrink-0 rounded-full ring-4 ${accentClasses[instance.accent]}`}
      />
      <strong className="truncate font-mono text-[8px] tracking-[0.02em]">
        {instance.label}
      </strong>
    </aside>
  );
}

export { DevInstanceBadge };
