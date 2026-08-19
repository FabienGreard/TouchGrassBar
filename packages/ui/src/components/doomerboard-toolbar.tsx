import type { CodingProvider } from "@touchgrass/contracts";

import { Button } from "./button";
import {
  PanelMenu,
  PanelMenuContent,
  PanelMenuRadioGroup,
  PanelMenuRadioItem,
  PanelMenuTrigger,
} from "./panel-menu";
import {
  SegmentedControl,
  SegmentedControlItem,
} from "./segmented-control";

const periodOptions = [
  { label: "Today", value: "today" },
  { label: "7 days", value: "week" },
  { label: "30 days", value: "month" },
] as const;

type QueryOption = { label: string; value: string };
type DoomerboardProvider = {
  displayName: string;
  provider: CodingProvider;
};
type DoomerboardAudience = "global" | "mine";
type CurrentProfile = {
  displayName: string;
  touchGrassId: string;
};
type CopyStatus = "copied" | "idle" | "unavailable";

function QuerySelector({
  label,
  onValueChange,
  options,
  value,
}: {
  label: string;
  onValueChange: (value: string) => void;
  options: readonly QueryOption[];
  value: string;
}) {
  const selectedLabel =
    options.find((option) => option.value === value)?.label ?? value;

  return (
    <PanelMenu>
      <PanelMenuTrigger asChild>
        <Button
          aria-label={`Select Leaderboard ${label}`}
          size="quiet"
          type="button"
          variant="ghost"
        >
          {selectedLabel}
        </Button>
      </PanelMenuTrigger>
      <PanelMenuContent align="end" side="top" sideOffset={9} size="compact">
        <PanelMenuRadioGroup onValueChange={onValueChange} value={value}>
          {options.map((option) => (
            <PanelMenuRadioItem key={option.value} value={option.value}>
              {option.label}
            </PanelMenuRadioItem>
          ))}
        </PanelMenuRadioGroup>
      </PanelMenuContent>
    </PanelMenu>
  );
}

function CurrentProfileAction({
  copyStatus,
  currentProfile,
  onCopyCurrentProfile,
}: {
  copyStatus: CopyStatus;
  currentProfile: CurrentProfile | null;
  onCopyCurrentProfile?: (() => void) | undefined;
}) {
  if (currentProfile === null) {
    return (
      <small
        aria-label="Current user profile unavailable"
        className="font-mono text-[7px] text-pearl-muted"
        data-slot="current-user-profile"
      >
        Profile unavailable
      </small>
    );
  }

  const profile = `${currentProfile.displayName}${currentProfile.touchGrassId}`;
  return (
    <span
      className="inline-flex min-w-0 items-center gap-0.5"
      data-slot="current-user-profile"
    >
      <Button
        aria-label={`Copy current user profile ${profile}`}
        className="max-w-[142px] rounded-[5px] font-mono text-[7px] font-medium"
        data-copy-status={copyStatus}
        data-slot="current-user-profile-action"
        disabled={onCopyCurrentProfile === undefined}
        onClick={onCopyCurrentProfile}
        size="quiet"
        title={
          copyStatus === "copied"
            ? "Copied"
            : copyStatus === "unavailable"
              ? "Copy unavailable"
              : "Copy profile"
        }
        type="button"
        variant="ghost"
      >
        <span className="truncate">{profile}</span>
      </Button>
      <span
        aria-live="polite"
        className="font-mono text-[7px] text-pearl-ink"
        data-copy-feedback={copyStatus}
      >
        {copyStatus === "copied"
          ? "Copied"
          : copyStatus === "unavailable"
            ? "Unavailable"
            : ""}
      </span>
    </span>
  );
}

function DoomerboardToolbar({
  audience,
  copyStatus = "idle",
  currentProfile,
  onAudienceChange,
  onCopyCurrentProfile,
  onPeriodChange,
  onProviderChange,
  period,
  provider,
  providers,
}: {
  audience: DoomerboardAudience;
  copyStatus?: CopyStatus;
  currentProfile: CurrentProfile | null;
  onAudienceChange: (audience: DoomerboardAudience) => void;
  onCopyCurrentProfile?: (() => void) | undefined;
  onPeriodChange: (period: string) => void;
  onProviderChange: (provider: string) => void;
  period: string;
  provider: string;
  providers: readonly DoomerboardProvider[];
}) {
  const providerOptions: QueryOption[] = [
    { label: "Combined", value: "combined" },
    ...providers.map(({ displayName, provider: providerId }) => ({
      label: displayName,
      value: providerId,
    })),
  ];

  return (
    <>
      <header className="flex items-center justify-between px-3.5 pt-3">
        <div className="flex min-w-0 items-center gap-1.5">
          <strong className="shrink-0 text-[10px]">Leaderboard</strong>
          <CurrentProfileAction
            copyStatus={copyStatus}
            currentProfile={currentProfile}
            onCopyCurrentProfile={onCopyCurrentProfile}
          />
        </div>
        <div className="flex items-center gap-0.5 text-pearl-muted">
          <QuerySelector
            label="period"
            onValueChange={onPeriodChange}
            options={periodOptions}
            value={period}
          />
          <span aria-hidden="true" className="text-[8px]">·</span>
          <QuerySelector
            label="provider"
            onValueChange={onProviderChange}
            options={providerOptions}
            value={provider}
          />
        </div>
      </header>
      <div className="mx-3.5 mt-2">
        <SegmentedControl
          aria-label="Leaderboard audience"
          onValueChange={(value) =>
            onAudienceChange(value as DoomerboardAudience)
          }
          value={audience}
        >
          <SegmentedControlItem value="mine">Friends</SegmentedControlItem>
          <SegmentedControlItem value="global">Global</SegmentedControlItem>
        </SegmentedControl>
      </div>
    </>
  );
}

export { DoomerboardToolbar };
export type {
  CopyStatus as DoomerboardCopyStatus,
  CurrentProfile as DoomerboardCurrentProfile,
  DoomerboardAudience,
  DoomerboardProvider,
};
