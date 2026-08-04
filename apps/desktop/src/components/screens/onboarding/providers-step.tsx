import { CodingProviderAccessCard } from "@/components/coding-provider-access";
import type { CodingProviderAccessState } from "@/components/coding-provider-access-state";

function ProvidersStep({
  busy = false,
  codexState,
  onCheckProvider,
  providerState,
}: {
  busy?: boolean;
  codexState: CodingProviderAccessState;
  onCheckProvider?: ((provider: "claude" | "codex") => void) | undefined;
  providerState: CodingProviderAccessState;
}) {
  return (
    <div className="grid gap-3">
      <CodingProviderAccessCard
        busy={busy}
        onCheck={
          onCheckProvider === undefined
            ? undefined
            : () => onCheckProvider("codex")
        }
        provider="codex"
        state={codexState}
      />
      <CodingProviderAccessCard
        busy={busy}
        onCheck={
          onCheckProvider === undefined
            ? undefined
            : () => onCheckProvider("claude")
        }
        provider="claude"
        state={providerState}
      />
    </div>
  );
}

export { ProvidersStep };
