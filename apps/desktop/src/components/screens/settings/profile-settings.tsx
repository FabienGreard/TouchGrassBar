import { Button, Input, ProfileCard } from "@touchgrass/ui";
import type { ProfileProvisioningStatus } from "@touchgrass/contracts";
import { useEffect, useRef } from "react";

import { ProfileEditor } from "./profile-editor";
import {
  focusAndSelectRecoveryInput,
  maskRecoveryKeySuffix,
} from "./recovery-key-input";

type SettingsProfile = {
  displayName: string;
  recoveryKeySuffix: string | null;
  touchGrassId: string;
};

function ProfileSettings({
  onDisplayNameChange,
  onHideRecoveryKey,
  onRevealRecoveryKey,
  onStartRecovery,
  pendingDisplayName,
  profile = null,
  profileProvisioning = "not-authorized",
  recoveryKey = null,
  revealingRecoveryKey = false,
}: {
  onDisplayNameChange?: ((displayName: string) => void) | undefined;
  onHideRecoveryKey?: (() => void) | undefined;
  onRevealRecoveryKey?: (() => void) | undefined;
  onStartRecovery?: (() => void) | undefined;
  pendingDisplayName?: string | null | undefined;
  profile?: SettingsProfile | null | undefined;
  profileProvisioning?: ProfileProvisioningStatus | undefined;
  recoveryKey?: string | null | undefined;
  revealingRecoveryKey?: boolean | undefined;
}) {
  const pendingName = pendingDisplayName?.trim() || "Your Profile";
  const pendingInitial = pendingName.slice(0, 1).toUpperCase();
  const recoveryInput = useRef<HTMLInputElement>(null);
  const recoveryKeyVisible = recoveryKey !== null;
  const recoveryKeyUnavailable =
    revealingRecoveryKey ||
    (recoveryKeyVisible
      ? onHideRecoveryKey === undefined
      : onRevealRecoveryKey === undefined);

  useEffect(() => {
    if (recoveryKey === null || onHideRecoveryKey === undefined) return;
    focusAndSelectRecoveryInput(recoveryInput.current);
    return () => onHideRecoveryKey();
  }, [onHideRecoveryKey, recoveryKey]);

  function toggleRecoveryKey() {
    if (recoveryKeyVisible) {
      onHideRecoveryKey?.();
      return;
    }
    onRevealRecoveryKey?.();
  }

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
          {profile === null || profile.recoveryKeySuffix === null ? (
            <div data-profile-recovery-key-state="unavailable">
              <strong className="block text-[12px]">
                Recovery Key unavailable
              </strong>
              <small className="mt-0.5 block text-[9px] text-sheet-muted">
                {profile === null
                  ? "Available when this Profile is ready."
                  : "Keychain status is not available."}
              </small>
            </div>
          ) : (
            <div>
              <strong className="block text-[12px]">Recovery Key</strong>
              <small className="mt-0.5 block text-[9px] text-sheet-muted">
                Stored in this Mac’s Keychain.
              </small>
              <div className="relative mt-3" data-slot="masked-recovery-key">
                <Input
                  aria-label="Recovery Key"
                  autoComplete="off"
                  autoFocus={recoveryKeyVisible}
                  className="h-12 rounded-[12px] pr-[72px] font-mono tracking-[0.06em]"
                  disabled={recoveryKeyUnavailable}
                  onFocus={(event) => event.currentTarget.select()}
                  readOnly
                  ref={recoveryInput}
                  spellCheck={false}
                  type="text"
                  value={
                    recoveryKey ??
                    maskRecoveryKeySuffix(profile.recoveryKeySuffix)
                  }
                />
                <Button
                  aria-expanded={recoveryKeyVisible}
                  aria-label={`${recoveryKeyVisible ? "Hide" : "View"} Recovery Key`}
                  className="absolute top-1/2 right-2 -translate-y-1/2"
                  disabled={recoveryKeyUnavailable}
                  onClick={toggleRecoveryKey}
                  size="quiet"
                  type="button"
                  variant="ghost"
                >
                  {revealingRecoveryKey
                    ? "Opening…"
                    : recoveryKeyVisible
                      ? "Hide"
                      : "View"}
                </Button>
              </div>
            </div>
          )}
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
