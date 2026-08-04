import { CodingProviderAccessCard } from "@/components/coding-provider-access";
import type { CodingProviderAccessState } from "@/components/coding-provider-access-state";

function ProvidersStep({
  codexState,
  providerState,
}: {
  codexState: CodingProviderAccessState;
  providerState: CodingProviderAccessState;
}) {
  return (
    <div className="grid gap-3">
      <CodingProviderAccessCard provider="codex" state={codexState} />
      <CodingProviderAccessCard provider="claude" state={providerState} />
    </div>
  );
}

export { ProvidersStep };
