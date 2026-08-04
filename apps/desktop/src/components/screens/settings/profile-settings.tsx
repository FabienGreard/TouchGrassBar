import { ProfileCard } from "@touchgrass/ui";

import { ProfileEditor } from "./profile-editor";

type SettingsProfile = {
  displayName: string;
  touchGrassId: string;
};

function ProfileSettings({
  onDisplayNameChange,
  profile = null,
}: {
  onDisplayNameChange?: ((displayName: string) => void) | undefined;
  profile?: SettingsProfile | null | undefined;
}) {
  return (
    <div className="grid gap-3" data-slot="profile-settings">
      {profile === null ? (
        <ProfileCard data-profile-state="unavailable">
          <strong className="block text-[12px]">Profile unavailable</strong>
          <small className="mt-1 block text-[9px] leading-4 text-sheet-muted">
            Profile state is not connected in this build.
          </small>
        </ProfileCard>
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
          Recovery and key access require a secure macOS sheet and are not
          available in this build.
        </small>
      </div>
    </div>
  );
}

export { ProfileSettings };
export type { SettingsProfile };
