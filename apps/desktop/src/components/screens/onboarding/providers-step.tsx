import type { CodingProvider } from "@touchgrass/contracts";

import { CodingProviderAccessCard } from "@/components/provider-access/card";
import type { CodingProviderAccessPresentation } from "@/components/provider-access/presentation";

function ProvidersStep({
  busy = false,
  onCheckProvider,
  providers,
}: {
  busy?: boolean;
  onCheckProvider?: ((provider: CodingProvider) => void) | undefined;
  providers: readonly CodingProviderAccessPresentation[];
}) {
  return (
    <div className="grid gap-3">
      {providers.map((provider) => (
        <CodingProviderAccessCard
          busy={busy}
          displayName={provider.displayName}
          key={provider.provider}
          onCheck={
            onCheckProvider === undefined ? undefined : () => onCheckProvider(provider.provider)
          }
          provider={provider.provider}
          state={provider.state}
        />
      ))}
    </div>
  );
}

export { ProvidersStep };
