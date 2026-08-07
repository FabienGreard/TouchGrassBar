import type { ProviderPresence } from "@touchgrass/contracts";

// Shared presentation policy for provider access in Settings and Onboarding.

type CodingProviderAccessState =
  "detected" | "needs-access" | "not-installed" | "ready" | "unavailable";

const codingProviderAccessStates = [
  { key: "detected", label: "Detected" },
  { key: "ready", label: "Ready" },
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

function providerAccessPresentations(
  providers: readonly ProviderPresence[] | undefined,
): CodingProviderAccessPresentation[] {
  return (providers ?? []).map((provider) => ({
    displayName: provider.displayName,
    provider: provider.provider,
    state: providerAccessStateFromPresence(provider),
  }));
}

export { codingProviderAccessStates, providerAccessPresentations };
export type {
  CodingProviderAccessPresentation,
  CodingProviderAccessState,
};
