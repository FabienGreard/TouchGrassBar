import type { ProviderSnapshot } from "@touchgrass/contracts";

import { DoomerboardPreview } from "@/components/panel/doomerboard-preview";
import { ProviderCard } from "@/components/panel/provider-card";
import { UsageOverview } from "@/components/panel/usage-overview";

const loadingProviders = [
  {
    availability: "current",
    observedAt: "1970-01-01T00:00:00.000Z",
    provider: "codex",
    quotaLanes: [
      {
        allowance: null,
        label: "Weekly limit",
        remaining: null,
        resetAt: null,
        unit: "percent",
      },
      {
        allowance: null,
        label: "5-hour limit",
        remaining: null,
        resetAt: null,
        unit: "percent",
      },
    ],
  },
  {
    availability: "current",
    observedAt: "1970-01-01T00:00:00.000Z",
    provider: "claude",
    quotaLanes: [
      {
        allowance: null,
        label: "Weekly limit",
        remaining: null,
        resetAt: null,
        unit: "percent",
      },
      {
        allowance: null,
        label: "5-hour limit",
        remaining: null,
        resetAt: null,
        unit: "percent",
      },
    ],
  },
] as const satisfies readonly ProviderSnapshot[];

const loadingUsage = {
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
        <DoomerboardPreview />
      </div>
    </div>
  );
}

export { LoadingPanel };
