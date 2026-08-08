import type {
  ProviderPresentation,
  UpdateState,
  UsagePeriods,
} from "@touchgrass/contracts";
import type { Meta, StoryObj } from "@storybook/react-vite";

import {
  Brand,
  Button,
  CodingProviderAccessCard,
  DoomerboardRankings,
  DoomerboardToolbar,
  EllipsisIcon,
  NativeSheetSurface,
  PanelShell,
  ProviderCard,
  SettingsToggleRow,
  UpdatesSettings,
  UsageOverview,
} from "../index";

const observedAt = "2026-08-07T12:00:00.000Z";
const usage: UsagePeriods = {
  scanStatus: "complete",
  sevenDays: {
    apiEquivalentCostBasis: "published-provider-pricing",
    apiEquivalentCostCoveragePercent: null,
    apiEquivalentCostQuality: "reconciled",
    apiEquivalentCostUsd: 214.96,
    availability: "current",
    coverage: "complete",
    evidenceBasis: "provider-reported",
    observedAt,
    observedTokens: 71_400_000,
    trendPercent: 14,
  },
  thirtyDays: {
    apiEquivalentCostBasis: "published-provider-pricing",
    apiEquivalentCostCoveragePercent: null,
    apiEquivalentCostQuality: "reconciled",
    apiEquivalentCostUsd: 856.73,
    availability: "current",
    coverage: "complete",
    evidenceBasis: "provider-reported",
    observedAt,
    observedTokens: 284_600_000,
    trendPercent: 22,
  },
  today: {
    apiEquivalentCostBasis: "published-provider-pricing",
    apiEquivalentCostCoveragePercent: null,
    apiEquivalentCostQuality: "reconciled",
    apiEquivalentCostUsd: 38.61,
    availability: "current",
    coverage: "complete",
    evidenceBasis: "provider-reported",
    observedAt,
    observedTokens: 12_800_000,
    trendPercent: -8,
  },
};

const providers: ProviderPresentation[] = [
  {
    displayName: "Codex",
    presence: "detected",
    provider: "codex",
    quota: {
      availability: "current",
      observedAt,
      provider: "codex",
      quotaLanes: [
        {
          allowance: 100,
          label: "Weekly limit",
          remaining: 74,
          resetAt: "2026-08-12T14:00:00.000Z",
          unit: "percent",
        },
        {
          allowance: 100,
          label: "5-hour limit",
          remaining: 62,
          resetAt: "2026-08-08T14:40:00.000Z",
          unit: "percent",
        },
      ],
    },
    usage,
  },
];

const updateState: UpdateState = {
  automaticChecksEnabled: true,
  contractVersion: 2,
  currentVersion: "1.3.2",
  onlineFeaturesPaused: false,
  update: { status: "available", version: "1.4.0" },
};

const rows = [
  {
    displayName: "laura",
    note: "ABSOLUTELY FINE",
    rank: 1,
    tokenScore: "18.2M",
    touchGrassId: "#TG-4COLD7",
  },
  {
    displayName: "Fabien",
    note: "YOU",
    rank: 2,
    tokenScore: "12.8M",
    touchGrassId: "#TG-7K4P9D",
  },
  {
    displayName: "max",
    note: "STILL ONLINE",
    rank: 3,
    tokenScore: "9.1M",
    touchGrassId: "#TG-BURN42",
  },
] as const;

const noOp = () => undefined;

const meta = {
  parameters: { layout: "centered" },
  title: "Product/Shared compositions",
} satisfies Meta;

export default meta;
type Story = StoryObj<typeof meta>;

export const CompactPanel: Story = {
  render: () => (
    <PanelShell>
      <header className="flex items-center justify-between border-b border-pearl-line bg-panel-header px-4 pt-[15px] pb-3">
        <div className="flex items-center gap-2.5">
          <Brand />
          <small className="border-l border-pearl-line pl-2.5 text-[10px] text-pearl-muted">
            Synced locally
          </small>
        </div>
        <Button aria-label="Open panel menu" size="icon" variant="ghost">
          <EllipsisIcon aria-hidden="true" />
        </Button>
      </header>
      <ProviderCard presentation={providers[0]!} />
      <UsageOverview usage={usage} />
      <section className="pb-2">
        <DoomerboardToolbar
          audience="global"
          currentProfile={{
            displayName: "Fabien",
            touchGrassId: "#TG-7K4P9D",
          }}
          onAudienceChange={noOp}
          onCopyCurrentProfile={noOp}
          onPeriodChange={noOp}
          onProviderChange={noOp}
          period="today"
          provider="combined"
          providers={providers}
        />
        <div className="mt-3 h-[180px]">
          <DoomerboardRankings rows={rows} />
        </div>
      </section>
    </PanelShell>
  ),
};

export const ProviderAccess: Story = {
  render: () => (
    <NativeSheetSurface className="w-[620px] max-w-full p-8">
      <CodingProviderAccessCard
        displayName="Codex"
        provider="codex"
        state="detected"
      />
    </NativeSheetSurface>
  ),
};

export const UpdateRows: Story = {
  render: () => (
    <NativeSheetSurface className="grid w-[620px] max-w-full gap-6 p-8">
      <SettingsToggleRow
        checked={false}
        description="Start quietly in the menu bar."
        label="Open at login"
        onCheckedChange={noOp}
      />
      <UpdatesSettings
        autoUpdates
        onAutoUpdatesChange={noOp}
        onCheckForUpdates={noOp}
        onInstall={noOp}
        onOpenLatestDmg={noOp}
        onOpenSource={noOp}
        onRetry={noOp}
        state={updateState}
      />
    </NativeSheetSurface>
  ),
};
