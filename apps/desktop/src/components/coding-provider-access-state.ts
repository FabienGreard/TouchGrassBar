import type { ProviderPresence } from "@touchgrass/contracts";

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
  providers: readonly ProviderPresence[] | undefined,
  provider: ProviderPresence["provider"],
): CodingProviderAccessState {
  const status = providers?.find((item) => item.provider === provider)?.status;
  if (status === "detected") return "detected";
  if (status === "not-detected") return "not-installed";
  return "unavailable";
}

export { codingProviderAccessStates, providerAccessStateFromPresence };
export type { CodingProviderAccessState };
