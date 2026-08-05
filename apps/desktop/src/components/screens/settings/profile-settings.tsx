import { Button, ProfileCard } from "@touchgrass/ui";
import type { ProfileProvisioningStatus } from "@touchgrass/contracts";

import { ProfileEditor } from "./profile-editor";

type SettingsProfile = {
  displayName: string;
  touchGrassId: string;
};

function ProfileSettings({
  onDisplayNameChange,
  onRevealRecoveryKey,
  onStartRecovery,
  pendingDisplayName,
  profile = null,
  profileProvisioning = "not-authorized",
  revealingRecoveryKey = false,
}: {
  onDisplayNameChange?: ((displayName: string) => void) | undefined;
  onRevealRecoveryKey?: (() => void) | undefined;
  onStartRecovery?: (() => void) | undefined;
  pendingDisplayName?: string | null | undefined;
  profile?: SettingsProfile | null | undefined;
  profileProvisioning?: ProfileProvisioningStatus | undefined;
  revealingRecoveryKey?: boolean | undefined;
}) {
  const pendingName = pendingDisplayName?.trim() || "Your Profile";
  const pendingInitial = pendingName.slice(0, 1).toUpperCase();
  const recoveryKeyUnavailable =
    profile === null ||
    onRevealRecoveryKey === undefined ||
    revealingRecoveryKey;

  return (
    <div className="grid gap-3" data-slot="profile-settings">
      {profile === null ? (
        profileProvisioning === "profile-pending" ? (
          <ProfileCard
            avatarLabel={pendingInitial}
            data-profile-state="profile-pending"
            displayName={
              <strong className="mt-0.5 block truncate text-[13px]">
                {pendingName}
              </strong>
            }
            touchGrassId={
              <strong className="mt-1 block font-mono text-[10px]">
                Profile Pending
              </strong>
            }
            touchGrassIdDescription="Assigned automatically when Profile services are available. Local provider utility remains available."
          />
        ) : (
          <ProfileCard data-profile-state="unavailable">
            <strong className="block text-[12px]">Profile unavailable</strong>
            <small className="mt-1 block text-[9px] leading-4 text-sheet-muted">
              Profile state is not connected in this build.
            </small>
          </ProfileCard>
        )
      ) : (
        <ProfileEditor
          displayName={profile.displayName}
          onDisplayNameChange={onDisplayNameChange}
          touchGrassId={profile.touchGrassId}
        />
      )}
      <section
        className="mt-1 border-t border-sheet-line pt-5"
        data-profile-recovery-state="native-required"
      >
        <h2 className="m-0 text-[14px]">Recovery</h2>
        <p className="mt-1 mb-4 text-[10px] leading-4 text-sheet-muted">
          Manage recovery for this Profile.
        </p>
        <div className="grid gap-3">
          <div>
            <strong className="block text-[12px]">Recovery Key</strong>
            <small className="mt-0.5 block text-[9px] text-sheet-muted">
              Stored in this Mac’s Keychain.
            </small>
            <div
              aria-disabled={recoveryKeyUnavailable}
              className="mt-3 flex h-9 items-center rounded-[8px] border border-input bg-white px-3 shadow-control aria-disabled:opacity-50 contrast-more:border-pearl-ink"
              data-slot="masked-recovery-key"
            >
              <span
                aria-hidden="true"
                className="font-mono text-[11px] tracking-[0.12em] text-pearl-muted"
              >
                ••••••••••••••••
              </span>
              <Button
                aria-label="View Recovery Key"
                className="ml-auto"
                disabled={recoveryKeyUnavailable}
                onClick={onRevealRecoveryKey}
                size="quiet"
                type="button"
                variant="ghost"
              >
                {revealingRecoveryKey ? "Opening…" : "View"}
              </Button>
            </div>
          </div>
          <div className="flex items-center justify-between gap-6 border-t border-sheet-line pt-4">
            <span>
              <strong className="block text-[11px]">
                Recover from another Mac
              </strong>
              <small className="mt-0.5 block text-[9px] leading-4 text-sheet-muted">
                Enter the Recovery Key stored on your other Mac to restore its
                Profile here.
              </small>
            </span>
            <Button
              disabled={onStartRecovery === undefined}
              onClick={onStartRecovery}
              type="button"
            >
              Enter Recovery Key…
            </Button>
          </div>
        </div>
      </section>
    </div>
  );
}

export { ProfileSettings };
export type { SettingsProfile };
