import {
  Brand,
  NativeWindow,
  NativeWindowContent,
  NativeWindowNav,
  NativeWindowNavItem,
  NativeWindowSidebar,
} from "@touchgrass/ui";
import type { CodingProvider, ProfileProvisioningStatus, UpdateState } from "@touchgrass/contracts";
import { useState } from "react";

import { CodingProviderAccessCard } from "@/components/provider-access/card";
import type { SettingsProviderAccessPresentation } from "@/components/provider-access/presentation";

import { ProfileSettings, type SettingsProfile } from "./profile-settings";
import {
  resolveSettingsSectionHash,
  settingsSections,
  type SettingsSection,
} from "./settings-section";
import { SettingsToggleRow } from "./settings-toggle-row";
import { UpdatesSettings } from "./updates-settings";

const settingsSectionDetails: Record<SettingsSection, { description: string; label: string }> = {
  general: {
    description: "Startup, updates, and app information.",
    label: "General",
  },
  providers: {
    description: "Local coding-provider connections and total inclusion.",
    label: "Providers",
  },
  profile: {
    description: "Your public TouchGrass profile and recovery.",
    label: "Profile",
  },
};
type SettingsScreenProps = {
  autoUpdates?: boolean | null | undefined;
  busyProviders?: boolean | undefined;
  launchAtLogin?: boolean | null | undefined;
  launchAtLoginSaving?: boolean | undefined;
  onAutoUpdatesChange?: ((value: boolean) => void) | undefined;
  onCheckProviders?: (() => void) | undefined;
  onCheckForUpdates?: (() => void) | undefined;
  onInstallUpdate?: (() => void) | undefined;
  onLaunchAtLoginChange?: ((value: boolean) => void) | undefined;
  onOpenLatestDmg?: (() => void) | undefined;
  onOpenSource?: (() => void) | undefined;
  onProfileDisplayNameChange?:
    | ((displayName: string) => boolean | Promise<boolean> | void)
    | undefined;
  onProviderEnabledChange?: ((provider: CodingProvider, enabled: boolean) => void) | undefined;
  onHideRecoveryKey?: (() => void) | undefined;
  onRevealRecoveryKey?: (() => void) | undefined;
  onStartRecovery?: (() => void) | undefined;
  onRetryUpdate?: (() => void) | undefined;
  onSectionChange?: ((section: SettingsSection) => void) | undefined;
  pendingDisplayName?: string | null | undefined;
  profile?: SettingsProfile | null | undefined;
  profileProvisioning?: ProfileProvisioningStatus | undefined;
  recoveryFailed?: boolean | undefined;
  providers?: readonly SettingsProviderAccessPresentation[] | undefined;
  recoveryKey?: string | null | undefined;
  revealingRecoveryKey?: boolean | undefined;
  savingProviders?: readonly CodingProvider[] | undefined;
  section?: SettingsSection | undefined;
  updateActionPending?: boolean | undefined;
  updateState?: UpdateState | null | undefined;
};

function SettingsScreen({
  autoUpdates = null,
  busyProviders = false,
  launchAtLogin = null,
  launchAtLoginSaving = false,
  onAutoUpdatesChange,
  onCheckProviders,
  onCheckForUpdates,
  onInstallUpdate,
  onLaunchAtLoginChange,
  onOpenLatestDmg,
  onOpenSource,
  onProfileDisplayNameChange,
  onProviderEnabledChange,
  onHideRecoveryKey,
  onRevealRecoveryKey,
  onStartRecovery,
  onRetryUpdate,
  onSectionChange,
  pendingDisplayName = null,
  profile = null,
  profileProvisioning = "not-authorized",
  providers = [],
  recoveryFailed = false,
  recoveryKey = null,
  revealingRecoveryKey = false,
  savingProviders = [],
  section: controlledSection,
  updateActionPending = false,
  updateState = null,
}: SettingsScreenProps) {
  const [localSection, setLocalSection] = useState<SettingsSection>(() =>
    typeof window === "undefined" ? "general" : resolveSettingsSectionHash(window.location.hash),
  );
  const section = controlledSection ?? localSection;
  const savingProviderSet = new Set(savingProviders);
  const selectSection = (nextSection: SettingsSection) => {
    if (controlledSection === undefined) setLocalSection(nextSection);
    onSectionChange?.(nextSection);
    if (typeof window !== "undefined") {
      window.history.replaceState(null, "", `#settings-${nextSection}`);
    }
  };

  const detail = settingsSectionDetails[section];

  return (
    <NativeWindow className="relative h-screen min-h-0 w-screen max-w-none min-w-0 min-[680px]:grid-cols-[220px_minmax(0,1fr)]">
      <NativeWindowSidebar className="h-full min-h-0 overflow-hidden px-4 py-7">
        <Brand className="px-2" />
        <NativeWindowNav aria-label="Settings sections" className="mt-8">
          {settingsSections.map((item) => (
            <NativeWindowNavItem asChild key={item}>
              <button
                aria-current={section === item ? "page" : undefined}
                className="cursor-pointer border-0 bg-transparent text-left"
                onClick={() => selectSection(item)}
                type="button"
              >
                {settingsSectionDetails[item].label}
              </button>
            </NativeWindowNavItem>
          ))}
        </NativeWindowNav>
        <small className="mt-auto px-2 font-mono text-[9px] text-sheet-muted">
          Version {updateState?.currentVersion ?? "unavailable"}
        </small>
      </NativeWindowSidebar>

      <NativeWindowContent className="h-full min-h-0 overflow-y-auto px-12 py-12">
        <div className="mx-auto max-w-[720px]">
          <h1 className="m-0 text-[32px] tracking-[-0.045em]">{detail.label}</h1>
          <p className="mt-2 mb-9 text-[12px] text-sheet-muted">{detail.description}</p>
          {section === "general" ? (
            <div className="grid gap-6">
              <SettingsToggleRow
                checked={launchAtLogin ?? false}
                description={
                  launchAtLogin === null
                    ? "Not connected in this build."
                    : "Start quietly in the menu bar."
                }
                disabled={
                  launchAtLogin === null ||
                  launchAtLoginSaving ||
                  onLaunchAtLoginChange === undefined
                }
                label="Open at login"
                onCheckedChange={onLaunchAtLoginChange}
              />
              <section className="border-t border-sheet-line pt-5">
                <h2 className="m-0 text-[14px]">Updates</h2>
                <p className="mt-1 mb-4 text-[10px] leading-4 text-sheet-muted">
                  Version and update checks.
                </p>
                <UpdatesSettings
                  actionPending={updateActionPending}
                  autoUpdates={autoUpdates}
                  onAutoUpdatesChange={onAutoUpdatesChange}
                  onCheckForUpdates={onCheckForUpdates}
                  onInstall={onInstallUpdate}
                  onOpenLatestDmg={onOpenLatestDmg}
                  onOpenSource={onOpenSource}
                  onRetry={onRetryUpdate}
                  state={updateState}
                />
              </section>
            </div>
          ) : null}
          {section === "providers" ? (
            <div className="grid gap-3">
              {providers.map((provider) => (
                <CodingProviderAccessCard
                  busy={busyProviders}
                  displayName={provider.displayName}
                  enabled={provider.enabled}
                  key={provider.provider}
                  onCheck={onCheckProviders}
                  onEnabledChange={
                    onProviderEnabledChange === undefined
                      ? undefined
                      : (enabled) => onProviderEnabledChange(provider.provider, enabled)
                  }
                  provider={provider.provider}
                  savingEnabled={savingProviderSet.has(provider.provider)}
                  state={provider.state}
                />
              ))}
            </div>
          ) : null}
          {section === "profile" ? (
            <ProfileSettings
              onDisplayNameChange={onProfileDisplayNameChange}
              onHideRecoveryKey={onHideRecoveryKey}
              onRevealRecoveryKey={onRevealRecoveryKey}
              onStartRecovery={onStartRecovery}
              pendingDisplayName={pendingDisplayName}
              profile={profile}
              profileProvisioning={profileProvisioning}
              recoveryFailed={recoveryFailed}
              recoveryKey={recoveryKey}
              revealingRecoveryKey={revealingRecoveryKey}
            />
          ) : null}
        </div>
      </NativeWindowContent>
    </NativeWindow>
  );
}

export { SettingsScreen };
export type { SettingsScreenProps };
