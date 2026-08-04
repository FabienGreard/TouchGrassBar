type CodingProviderAccessState =
  "needs-access" | "not-installed" | "ready" | "unavailable";

const codingProviderAccessStates = [
  { key: "ready", label: "Ready" },
  { key: "needs-access", label: "Needs access" },
  { key: "not-installed", label: "Not installed" },
  { key: "unavailable", label: "Unavailable" },
] as const;

export { codingProviderAccessStates };
export type { CodingProviderAccessState };
