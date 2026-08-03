import type { ReactNode } from "react";

export function PanelShell({ children }: { children: ReactNode }) {
  return (
    <main className="min-h-screen overflow-hidden rounded-2xl border border-ash-700 bg-ash-950 text-ash-100 shadow-2xl">
      {children}
    </main>
  );
}

export function MetricCard({
  label,
  value,
  detail,
}: {
  label: string;
  value: string;
  detail?: string;
}) {
  return (
    <section className="rounded-xl border border-ash-700 bg-ash-900 p-3">
      <p className="m-0 text-xs font-medium uppercase tracking-[0.13em] text-ash-400">
        {label}
      </p>
      <p className="mb-0 mt-1 text-xl font-semibold tabular-nums">{value}</p>
      {detail ? <p className="mb-0 mt-1 text-xs text-ash-400">{detail}</p> : null}
    </section>
  );
}

export function StatusPill({ children }: { children: ReactNode }) {
  return (
    <span className="inline-flex rounded-full bg-grass-400 px-2 py-1 text-[11px] font-semibold text-grass-950">
      {children}
    </span>
  );
}
