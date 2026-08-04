import type { ComponentProps } from "react";

import claudeLogo from "../assets/providers/claude.svg";
import codexLogo from "../assets/providers/codex-color.svg";
import { cn } from "#lib/utils";

type ProviderMarkProps = Omit<ComponentProps<"img">, "src"> & {
  provider: "claude" | "codex";
  size?: "default" | "large";
};

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
      src={provider === "codex" ? codexLogo : claudeLogo}
      {...props}
    />
  );
}

export { ProviderMark };
export type { ProviderMarkProps };
