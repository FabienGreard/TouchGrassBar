import {
  Button,
  DoomerboardRankings,
  DoomerboardToolbar,
  InviteIcon,
  RankingIcon,
} from "@touchgrass/ui";
import type {
  DoomerboardAudience,
  DoomerboardCurrentProfile,
  DoomerboardProvider,
  DoomerboardRow,
} from "@touchgrass/ui";

import { useCopyText } from "@/components/use-copy-text";
import {
  defaultDoomerboardQuery,
  type DoomerboardQuery,
} from "@/native-state/doomerboard-delivery";

const emptyProviders: readonly DoomerboardProvider[] = [];
const INSTALLATION_URL = "https://touchgrassbar.com";

function tokenmaxxerInvitationText(currentProfile: DoomerboardCurrentProfile) {
  const touchGrassId = currentProfile.touchGrassId.replace(/^#/, "");
  return `Add me on TouchGrassBar with TouchGrass ID ${touchGrassId}. Install TouchGrassBar at ${INSTALLATION_URL}.`;
}

function DoomerboardUnavailable({
  selectionUnavailable = false,
}: {
  selectionUnavailable?: boolean;
}) {
  return (
    <div
      aria-label="Leaderboard unavailable"
      className="mx-3.5 flex h-full flex-col items-center justify-center rounded-[12px] border border-dashed border-pearl-line bg-pearl-surface px-6 py-3.5 text-center shadow-surface contrast-more:border-pearl-ink"
    >
      <RankingIcon aria-hidden="true" size={20} />
      <strong className="mt-1.5 text-[10px]">Leaderboard unavailable</strong>
      <small className="mt-0.5 max-w-[260px] text-[8px] leading-3.5 text-pearl-muted contrast-more:text-pearl-ink">
        {selectionUnavailable
          ? "Scores are unavailable for this selection."
          : "Profile and synchronized scores are not ready."}
      </small>
    </div>
  );
}

function TokenmaxxersEmpty({
  onAddTokenmaxxer = () => undefined,
}: {
  onAddTokenmaxxer?: (() => void) | undefined;
}) {
  return (
    <div
      aria-label="My Tokenmaxxers empty"
      className="mx-3.5 flex h-full flex-col items-center justify-center rounded-[12px] border border-dashed border-pearl-line bg-pearl-surface px-6 py-3.5 text-center shadow-surface contrast-more:border-pearl-ink"
    >
      <InviteIcon aria-hidden="true" size={20} />
      <strong className="mt-1.5 text-[10px]">Your Leaderboard is lonely</strong>
      <small className="mt-0.5 max-w-[260px] text-[8px] leading-3.5 text-pearl-muted contrast-more:text-pearl-ink">
        Add Tokenmaxxers by TouchGrass ID to compare scores.
      </small>
      <div className="mt-2">
        <Button onClick={onAddTokenmaxxer} type="button">
          Add a Tokenmaxxer
        </Button>
      </div>
    </div>
  );
}

function Doomerboard({
  currentProfile = null,
  onAddTokenmaxxer = () => undefined,
  onSelectionChange = () => undefined,
  providers = emptyProviders,
  rows,
  selection = defaultDoomerboardQuery,
  tokenmaxxerRows,
}: {
  currentProfile?: DoomerboardCurrentProfile | null | undefined;
  onAddTokenmaxxer?: (() => void) | undefined;
  onSelectionChange?: ((selection: DoomerboardQuery) => void) | undefined;
  providers?: readonly DoomerboardProvider[] | undefined;
  rows?: readonly DoomerboardRow[] | undefined;
  selection?: DoomerboardQuery | undefined;
  tokenmaxxerRows?: readonly DoomerboardRow[] | undefined;
}) {
  const invitationText = currentProfile ? tokenmaxxerInvitationText(currentProfile) : "";
  const { copyStatus, copyText } = useCopyText(invitationText);
  const selectedRows = selection.audience === "global" ? rows : tokenmaxxerRows;
  const rowsEmpty = selectedRows !== undefined && selectedRows.length === 0;
  const period =
    selection.windowDays === 1 ? "today" : selection.windowDays === 7 ? "week" : "month";
  const updateAudience = (audience: DoomerboardAudience) =>
    onSelectionChange({ ...selection, audience });
  const updatePeriod = (nextPeriod: string) => {
    const windowDays =
      nextPeriod === "today" ? 1 : nextPeriod === "week" ? 7 : nextPeriod === "month" ? 30 : null;
    if (windowDays === null) return;
    onSelectionChange({ ...selection, windowDays });
  };
  const updateProvider = (scope: string) => {
    if (scope !== "claude" && scope !== "codex" && scope !== "combined") {
      return;
    }
    onSelectionChange({ ...selection, scope });
  };
  return (
    <section
      aria-label={
        selection.audience === "mine"
          ? rowsEmpty
            ? "My Tokenmaxxers empty"
            : selectedRows !== undefined
              ? "My Tokenmaxxers rankings"
              : "Leaderboard unavailable"
          : rowsEmpty
            ? "Leaderboard unavailable"
            : selectedRows !== undefined
              ? "Leaderboard rankings"
              : "Leaderboard unavailable"
      }
      className="pb-2"
    >
      <DoomerboardToolbar
        audience={selection.audience}
        copyStatus={copyStatus}
        currentProfile={currentProfile}
        onAudienceChange={updateAudience}
        onCopyCurrentProfile={currentProfile ? () => void copyText() : undefined}
        onPeriodChange={updatePeriod}
        onProviderChange={updateProvider}
        period={period}
        provider={selection.scope}
        providers={providers}
      />
      <div className="mt-3 h-[180px]" data-slot="doomerboard-viewport">
        {selection.audience === "mine" && rowsEmpty ? (
          <TokenmaxxersEmpty onAddTokenmaxxer={onAddTokenmaxxer} />
        ) : selection.audience === "global" && rowsEmpty ? (
          <DoomerboardUnavailable />
        ) : selectedRows !== undefined ? (
          <DoomerboardRankings rows={selectedRows} />
        ) : (
          <DoomerboardUnavailable
            selectionUnavailable={rows !== undefined || tokenmaxxerRows !== undefined}
          />
        )}
      </div>
    </section>
  );
}

export { Doomerboard, tokenmaxxerInvitationText, TokenmaxxersEmpty };
export type { DoomerboardCurrentProfile as CurrentProfile };
export type { DoomerboardRow } from "@touchgrass/ui";
