import { invoke } from "@tauri-apps/api/core";
import { CodingProviderAccessCard as ProviderAccessCard } from "@touchgrass/ui";
import { useState, type ComponentProps } from "react";

type Props = Omit<
  ComponentProps<typeof ProviderAccessCard>,
  "installationGuideFailed" | "onOpenInstallationGuide"
>;

function CodingProviderAccessCard(props: Props) {
  const [installationGuideFailed, setInstallationGuideFailed] = useState(false);
  const hasNativeRuntime = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

  const openInstallationGuide = () => {
    setInstallationGuideFailed(false);
    void invoke("open_provider_installation_guide", { provider: props.provider }).catch(() => {
      setInstallationGuideFailed(true);
    });
  };

  return (
    <ProviderAccessCard
      {...props}
      installationGuideFailed={installationGuideFailed}
      onOpenInstallationGuide={hasNativeRuntime ? openInstallationGuide : undefined}
    />
  );
}

export { CodingProviderAccessCard };
