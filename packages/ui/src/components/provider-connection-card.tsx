import type { ComponentProps, ReactNode } from "react";

import { cn } from "#lib/utils";
import { ProviderMark } from "./provider-mark";

type ProviderConnectionCardProps = Omit<ComponentProps<"article">, "children"> & {
  action?: ReactNode;
  description: ReactNode;
  detail?: ReactNode;
  label: ReactNode;
  provider: "claude" | "codex";
  status: ReactNode;
  statusTone?: "attention" | "neutral" | "ready";
};

function StatusPill({
  children,
  tone = "ready",
}: {
  children: ReactNode;
  tone?: "attention" | "neutral" | "ready";
}) {
  return (
    <small
      className={cn(
        "inline-flex w-fit items-center rounded-full border px-2 py-0 text-[8px] leading-4 font-semibold whitespace-nowrap",
        tone === "ready" && "border-board-tab-active-border bg-action text-accent-foreground",
        tone === "attention" && "border-[#d7bd83] bg-[#fff4d9] text-[#664914]",
        tone === "neutral" && "border-sheet-line bg-[#e2e4eb] text-[#484c59] shadow-surface",
      )}
      data-slot="status-pill"
    >
      {children}
    </small>
  );
}

function ProviderConnectionCard({
  action,
  className,
  description,
  detail,
  label,
  provider,
  status,
  statusTone = "ready",
  ...props
}: ProviderConnectionCardProps) {
  return (
    <article
      className={cn(
        "grid grid-cols-[auto_minmax(0,1fr)] items-start gap-x-4 gap-y-3 rounded-[12px] border border-sheet-line bg-white/42 px-5 py-4 shadow-surface",
        className,
      )}
      data-slot="provider-connection-card"
      {...props}
    >
      <span className="grid size-11 place-items-center rounded-[11px] border border-sheet-line bg-white/75">
        <ProviderMark provider={provider} size="large" />
      </span>
      <div className="flex h-full min-w-0 flex-col">
        <div className="flex flex-wrap items-center gap-2">
          <strong className="text-[13px]">{label}</strong>
          <StatusPill tone={statusTone}>{status}</StatusPill>
        </div>
        <small className="mt-1.5 block w-full text-[10px] leading-4 text-sheet-muted">
          {description}
        </small>
        {detail}
        {action === undefined ? null : (
          <div className="mt-auto flex items-center justify-end pt-1">{action}</div>
        )}
      </div>
    </article>
  );
}

export { ProviderConnectionCard };
export type { ProviderConnectionCardProps };
