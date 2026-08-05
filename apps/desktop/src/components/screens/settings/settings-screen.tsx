import {
  Brand,
  NativeWindow,
  NativeWindowContent,
  NativeWindowNav,
  NativeWindowNavItem,
  NativeWindowSidebar,
} from "@touchgrass/ui";
import type { ProfileProvisioningStatus } from "@touchgrass/contracts";
import { useState } from "react";

import { CodingProviderAccessCard } from "@/components/coding-provider-access";
import type { CodingProviderAccessState } from "@/components/coding-provider-access-state";

import { ProfileSettings, type SettingsProfile } from "./profile-settings";
import {
  resolveSettingsSectionHash,
  settingsSections,
  type SettingsSection,
} from "./settings-section";
import { SettingsToggleRow } from "./settings-toggle-row";
import { UpdatesSettings } from "./updates-settings";

const settingsSectionDetails: Record<
  SettingsSection,
  { description: string; label: string }
> = {
  general: {
    description: "Startup, updates, and app information.",
    label: "General",
  },
  providers: {
    description: "Local coding-provider connections and read status.",
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
  codexState?: CodingProviderAccessState | undefined;
  launchAtLogin?: boolean | null | undefined;
  launchAtLoginSaving?: boolean | undefined;
  onAutoUpdatesChange?: ((value: boolean) => void) | undefined;
  onCheckProviders?: (() => void) | undefined;
  onCheckForUpdates?: (() => void) | undefined;
  onLaunchAtLoginChange?: ((value: boolean) => void) | undefined;
  onOpenSource?: (() => void) | undefined;
  onProfileDisplayNameChange?: ((displayName: string) => void) | undefined;
  onStartRecovery?: (() => void) | undefined;
  onSectionChange?: ((section: SettingsSection) => void) | undefined;
  pendingDisplayName?: string | null | undefined;
  profile?: SettingsProfile | null | undefined;
  profileProvisioning?: ProfileProvisioningStatus | undefined;
  providerState?: CodingProviderAccessState | undefined;
  section?: SettingsSection | undefined;
};

function SettingsScreen({
  autoUpdates = null,
  busyProviders = false,
  codexState = "unavailable",
  launchAtLogin = null,
  launchAtLoginSaving = false,
  onAutoUpdatesChange,
  onCheckProviders,
  onCheckForUpdates,
  onLaunchAtLoginChange,
  onOpenSource,
  onProfileDisplayNameChange,
  onStartRecovery,
  onSectionChange,
  pendingDisplayName = null,
  profile = null,
  profileProvisioning = "not-authorized",
  providerState = "unavailable",
  section: controlledSection,
}: SettingsScreenProps) {
  const [localSection, setLocalSection] = useState<SettingsSection>(() =>
    typeof window === "undefined"
      ? "general"
      : resolveSettingsSectionHash(window.location.hash),
  );
  const section = controlledSection ?? localSection;
  const selectSection = (nextSection: SettingsSection) => {
    if (controlledSection === undefined) setLocalSection(nextSection);
    onSectionChange?.(nextSection);
    if (typeof window !== "undefined") {
      window.history.replaceState(null, "", `#settings-${nextSection}`);
    }
  };

  const detail = settingsSectionDetails[section];

  return (
    <NativeWindow className="relative h-screen min-h-0 w-screen min-w-0 max-w-none min-[680px]:grid-cols-[220px_minmax(0,1fr)]">
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
          Version 0.0.0
        </small>
      </NativeWindowSidebar>

      <NativeWindowContent className="h-full min-h-0 overflow-y-auto px-12 py-12">
        <div className="mx-auto max-w-[720px]">
          <h1 className="m-0 text-[32px] tracking-[-0.045em]">
            {detail.label}
          </h1>
          <p className="mt-2 mb-9 text-[12px] text-sheet-muted">
            {detail.description}
          </p>
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
                  autoUpdates={autoUpdates}
                  onAutoUpdatesChange={onAutoUpdatesChange}
                  onCheckForUpdates={onCheckForUpdates}
                  onOpenSource={onOpenSource}
                />
              </section>
            </div>
          ) : null}
          {section === "providers" ? (
            <div className="grid gap-3">
              <CodingProviderAccessCard
                busy={busyProviders}
                onCheck={onCheckProviders}
                provider="codex"
                state={codexState}
              />
              <CodingProviderAccessCard
                busy={busyProviders}
                onCheck={onCheckProviders}
                provider="claude"
                state={providerState}
              />
            </div>
          ) : null}
          {section === "profile" ? (
            <ProfileSettings
              onDisplayNameChange={onProfileDisplayNameChange}
              onStartRecovery={onStartRecovery}
              pendingDisplayName={pendingDisplayName}
              profile={profile}
              profileProvisioning={profileProvisioning}
            />
          ) : null}
        </div>
      </NativeWindowContent>
    </NativeWindow>
  );
}

export { SettingsScreen };
export type { SettingsScreenProps };
