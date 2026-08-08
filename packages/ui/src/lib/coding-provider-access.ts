import type {
  ProviderPresence,
  SettingsProvider,
} from "@touchgrass/contracts";

type CodingProviderAccessState =
  "detected" | "needs-access" | "not-installed" | "unavailable";

const codingProviderAccessStates = [
  { key: "detected", label: "Ready" },
  { key: "needs-access", label: "Needs access" },
  { key: "not-installed", label: "Not installed" },
  { key: "unavailable", label: "Unavailable" },
] as const;

function providerAccessStateFromPresence(
  provider: ProviderPresence,
): CodingProviderAccessState {
  if (provider.status === "detected") return "detected";
  if (provider.status === "not-detected") return "not-installed";
  return "unavailable";
}

type CodingProviderAccessPresentation = Pick<
  ProviderPresence,
  "displayName" | "provider"
> & {
  state: CodingProviderAccessState;
};

type SettingsProviderAccessPresentation = CodingProviderAccessPresentation &
  Pick<SettingsProvider, "enabled">;

function providerAccessPresentations(
  providers: readonly ProviderPresence[] | undefined,
): CodingProviderAccessPresentation[] {
  return (providers ?? []).map((provider) => ({
    displayName: provider.displayName,
    provider: provider.provider,
    state: providerAccessStateFromPresence(provider),
  }));
}

function settingsProviderAccessPresentations(
  providers: readonly SettingsProvider[] | undefined,
): SettingsProviderAccessPresentation[] {
  return (providers ?? []).map((provider) => ({
    displayName: provider.displayName,
    enabled: provider.enabled,
    provider: provider.provider,
    state: providerAccessStateFromPresence(provider),
  }));
}

export {
  codingProviderAccessStates,
  providerAccessPresentations,
  settingsProviderAccessPresentations,
};
export type {
  CodingProviderAccessPresentation,
  CodingProviderAccessState,
  SettingsProviderAccessPresentation,
};
