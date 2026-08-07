import type { ComponentProps } from "react";

import claudeLogo from "../assets/providers/claude.svg";
import codexLogo from "../assets/providers/codex-color.svg";
import { cn } from "#lib/utils";

type ProviderMarkProps = Omit<ComponentProps<"img">, "src"> & {
  provider: "claude" | "codex";
  size?: "default" | "large";
};

const providerLogos = {
  claude: claudeLogo,
  codex: codexLogo,
} satisfies Record<ProviderMarkProps["provider"], string>;

function ProviderMark({
  alt = "",
  className,
  provider,
  size = "default",
  ...props
}: ProviderMarkProps) {
  return (
    <img
      alt={alt}
      className={cn(
        "object-contain",
        size === "default" ? "size-[19px]" : "size-6",
        className,
      )}
      data-size={size}
      data-slot="provider-mark"
      src={providerLogos[provider]}
      {...props}
    />
  );
}

export { ProviderMark };
export type { ProviderMarkProps };
