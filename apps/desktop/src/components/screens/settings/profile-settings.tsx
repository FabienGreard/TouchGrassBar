import { Button, Input, ProfileCard } from "@touchgrass/ui";
import type { ProfileProvisioningStatus } from "@touchgrass/contracts";
import { useEffect, useRef } from "react";

import { useCopyText } from "@/components/use-copy-text";

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
  const recoveryInput = useRef<HTMLTextAreaElement>(null);
  const recoveryKeyVisible = recoveryKey !== null;
  const { copyStatus, copyText } = useCopyText(recoveryKey ?? "");
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
              {recoveryKeyVisible ? (
                <div
                  className="mt-3 rounded-[12px] border border-input bg-white shadow-control transition-[border-color,box-shadow] focus-within:border-pearl-focus focus-within:ring-3 focus-within:ring-pearl-focus/25 contrast-more:border-pearl-ink"
                  data-slot="revealed-recovery-key"
                >
                  <textarea
                    aria-label="Recovery Key"
                    autoComplete="off"
                    autoFocus
                    className="block min-h-14 w-full resize-none overflow-hidden whitespace-pre-wrap break-all border-0 bg-transparent px-4 pt-3 pb-1 font-mono text-[11px] leading-5 tracking-[0.04em] text-sheet-ink outline-none"
                    disabled={recoveryKeyUnavailable}
                    onFocus={(event) => event.currentTarget.select()}
                    readOnly
                    ref={recoveryInput}
                    rows={2}
                    spellCheck={false}
                    value={recoveryKey}
                    wrap="soft"
                  />
                  <div className="flex min-h-7 items-center justify-end gap-1.5 px-2 pb-2">
                    <span
                      aria-live="polite"
                      className="text-[8px] text-sheet-muted"
                      data-copy-feedback={copyStatus}
                    >
                      {copyStatus === "copied"
                        ? "Copied"
                        : copyStatus === "unavailable"
                          ? "Copy unavailable"
                          : ""}
                    </span>
                    <Button
                      aria-label="Copy Recovery Key"
                      data-copy-status={copyStatus}
                      disabled={recoveryKeyUnavailable}
                      onClick={() => void copyText()}
                      size="quiet"
                      type="button"
                      variant="ghost"
                    >
                      Copy
                    </Button>
                    <Button
                      aria-expanded
                      aria-label="Hide Recovery Key"
                      disabled={recoveryKeyUnavailable}
                      onClick={toggleRecoveryKey}
                      size="quiet"
                      type="button"
                      variant="ghost"
                    >
                      Hide
                    </Button>
                  </div>
                </div>
              ) : (
                <div
                  className="relative mt-3"
                  data-slot="masked-recovery-key"
                >
                  <Input
                    aria-label="Recovery Key"
                    autoComplete="off"
                    className="h-12 rounded-[12px] pr-[72px] font-mono tracking-[0.06em]"
                    disabled={recoveryKeyUnavailable}
                    onFocus={(event) => event.currentTarget.select()}
                    readOnly
                    spellCheck={false}
                    type="text"
                    value={maskRecoveryKeySuffix(profile.recoveryKeySuffix)}
                  />
                  <Button
                    aria-expanded={false}
                    aria-label="View Recovery Key"
                    className="absolute top-1/2 right-2 -translate-y-1/2"
                    disabled={recoveryKeyUnavailable}
                    onClick={toggleRecoveryKey}
                    size="quiet"
                    type="button"
                    variant="ghost"
                  >
                    {revealingRecoveryKey ? "Opening…" : "View"}
                  </Button>
                </div>
              )}
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
