import { Button, ProfileCard } from "@touchgrass/ui";
import type { ProfileProvisioningStatus } from "@touchgrass/contracts";

import { ProfileEditor } from "./profile-editor";

type SettingsProfile = {
  displayName: string;
  touchGrassId: string;
};

function ProfileSettings({
  onDisplayNameChange,
  onStartRecovery,
  pendingDisplayName,
  profile = null,
  profileProvisioning = "not-authorized",
}: {
  onDisplayNameChange?: ((displayName: string) => void) | undefined;
  onStartRecovery?: (() => void) | undefined;
  pendingDisplayName?: string | null | undefined;
  profile?: SettingsProfile | null | undefined;
  profileProvisioning?: ProfileProvisioningStatus | undefined;
}) {
  const pendingName = pendingDisplayName?.trim() || "Your Profile";
  const pendingInitial = pendingName.slice(0, 1).toUpperCase();

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
      <div className="px-1 py-1" data-profile-security-state="native-required">
        <strong className="block text-[11px]">Profile security</strong>
        <small className="mt-1 block text-[9px] leading-4 text-sheet-muted">
          Your Recovery Key is stored in the local macOS Keychain. TouchGrassBar
          shows it only in a secure native sheet when this Profile is created.
        </small>
        <div className="mt-4 border-t border-sheet-line pt-4">
          <strong className="block text-[11px]">
            Recover from another Mac
          </strong>
          <small className="mt-1 block text-[9px] leading-4 text-sheet-muted">
            Enter the Recovery Key stored on your other Mac to restore its
            Profile here.
          </small>
          <Button
            className="mt-3"
            disabled={onStartRecovery === undefined}
            onClick={onStartRecovery}
            type="button"
          >
            Enter Recovery Key…
          </Button>
        </div>
      </div>
    </div>
  );
}

export { ProfileSettings };
export type { SettingsProfile };
