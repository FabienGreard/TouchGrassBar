import type { ProviderSnapshot } from "@touchgrass/contracts";

import { Doomerboard } from "@/components/panel/doomerboard";
import { ProviderCard } from "@/components/panel/provider-card";
import { UsageOverview } from "@/components/panel/usage-overview";

const loadingProviders = [
  {
    availability: "unavailable",
    provider: "codex",
    quotaLanes: [],
  },
  {
    availability: "unavailable",
    provider: "claude",
    quotaLanes: [],
  },
] as const satisfies readonly ProviderSnapshot[];

const loadingUsage = {
  scanStatus: "unavailable",
  sevenDays: { availability: "unavailable" },
  thirtyDays: { availability: "unavailable" },
  today: { availability: "unavailable" },
} as const;

function LoadingPanel() {
  return (
    <div
      aria-busy="true"
      aria-label="Loading local provider state"
      data-slot="loading-panel"
      role="status"
    >
      <span className="sr-only">Reading the local snapshot…</span>
      <div
        aria-hidden="true"
        className="pointer-events-none animate-pulse motion-reduce:animate-none"
        inert
      >
        <div>
          {loadingProviders.map((provider) => (
            <ProviderCard key={provider.provider} provider={provider} />
          ))}
        </div>
        <UsageOverview usage={loadingUsage} />
        <Doomerboard />
      </div>
    </div>
  );
}

export { LoadingPanel };
