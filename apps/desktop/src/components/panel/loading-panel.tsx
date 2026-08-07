import type { ProviderPresentation } from "@touchgrass/contracts";

import { Doomerboard } from "@/components/panel/doomerboard";
import { ProviderCard } from "@/components/panel/provider-card";
import { UsageOverview } from "@/components/panel/usage-overview";

const loadingUsage = {
  scanStatus: "unavailable",
  sevenDays: { availability: "unavailable" },
  thirtyDays: { availability: "unavailable" },
  today: { availability: "unavailable" },
} as const;

const loadingProviders = [
  {
    displayName: "Codex",
    presence: "unavailable",
    provider: "codex",
    quota: { availability: "unavailable", provider: "codex", quotaLanes: [] },
    usage: loadingUsage,
  },
  {
    displayName: "Claude",
    presence: "unavailable",
    provider: "claude",
    quota: {
      availability: "unavailable",
      provider: "claude",
      quotaLanes: [],
    },
    usage: loadingUsage,
  },
] as const satisfies readonly ProviderPresentation[];

type LoadingPanelProps = {
  loading?: boolean;
};

function LoadingPanel({ loading = true }: LoadingPanelProps) {
  return (
    <div
      aria-busy={loading || undefined}
      aria-label={
        loading ? "Loading local provider state" : "Local provider state unavailable"
      }
      data-slot="loading-panel"
      role={loading ? "status" : undefined}
    >
      {loading ? (
        <span className="sr-only">Reading the local snapshot…</span>
      ) : null}
      <div
        aria-hidden="true"
        className={
          loading
            ? "pointer-events-none animate-pulse motion-reduce:animate-none"
            : "pointer-events-none"
        }
        inert
      >
        <div>
          {loadingProviders.map((provider) => (
            <ProviderCard
              key={provider.provider}
              presentation={provider}
            />
          ))}
        </div>
        <UsageOverview usage={loadingUsage} />
        <Doomerboard />
      </div>
    </div>
  );
}

export { LoadingPanel };
export type { LoadingPanelProps };
