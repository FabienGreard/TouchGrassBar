type SettingsSection = "general" | "profile" | "providers";

const settingsSections = ["general", "providers", "profile"] as const;

function resolveSettingsSectionHash(hash: string): SettingsSection {
  const candidate = hash.replace("#settings-", "");
  return settingsSections.includes(candidate as SettingsSection)
    ? (candidate as SettingsSection)
    : "general";
}

export { resolveSettingsSectionHash, settingsSections };
export type { SettingsSection };
