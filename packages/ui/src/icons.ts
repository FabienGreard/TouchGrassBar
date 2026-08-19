import {
  ArrowExpand01Icon as HugeArrowExpand01Icon,
  ArrowShrink02Icon as HugeArrowShrink02Icon,
  CheckIcon as HugeCheckIcon,
  Download04Icon as HugeDownloadIcon,
  GripVerticalIcon as HugeGripVerticalIcon,
  MoreHorizontalCircle01Icon,
  MoreHorizontalIcon,
  RankingIcon as HugeRankingIcon,
  Refresh03Icon as HugeRefreshIcon,
  Settings01Icon,
  UserAdd01Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon, type HugeiconsIconProps, type IconSvgElement } from "@hugeicons/react";
import { createElement, forwardRef } from "react";

import { cn } from "./lib/utils";

type TouchGrassIconProps = Omit<
  HugeiconsIconProps,
  "color" | "icon" | "primaryColor" | "secondaryColor"
> & {
  color?: string;
  spin?: boolean;
  tone?: "default" | "muted" | "primary" | "unavailable";
};

type TouchGrassIcon = ReturnType<typeof createIcon>;

function createIcon(icon: IconSvgElement, displayName: string) {
  const TouchGrassIcon = forwardRef<SVGSVGElement, TouchGrassIconProps>(
    (
      {
        className,
        color = "currentColor",
        size = 24,
        spin = false,
        strokeWidth = 1.7,
        tone = "default",
        ...props
      },
      ref,
    ) =>
      createElement(HugeiconsIcon, {
        ...props,
        className: cn(
          tone === "default" && "text-pearl-ink",
          tone === "muted" && "text-pearl-muted",
          tone === "primary" && "text-primary",
          tone === "unavailable" && "text-usage-unavailable",
          spin && "animate-spin motion-reduce:animate-none",
          className,
        ),
        color,
        "data-icon-provider": "hugeicons",
        "data-icon-spin": spin || undefined,
        "data-icon-stroke-width": strokeWidth,
        "data-icon-tone": tone,
        icon,
        ref,
        size,
        strokeWidth,
      } as HugeiconsIconProps & { ref: typeof ref }),
  );
  TouchGrassIcon.displayName = displayName;
  return TouchGrassIcon;
}

const CheckIcon: TouchGrassIcon = createIcon(HugeCheckIcon, "CheckIcon");
const ArrowExpand01Icon: TouchGrassIcon = createIcon(HugeArrowExpand01Icon, "ArrowExpand01Icon");
const ArrowShrink02Icon: TouchGrassIcon = createIcon(HugeArrowShrink02Icon, "ArrowShrink02Icon");
const DownloadIcon: TouchGrassIcon = createIcon(HugeDownloadIcon, "DownloadIcon");
const GripVerticalIcon: TouchGrassIcon = createIcon(HugeGripVerticalIcon, "GripVerticalIcon");
const ProviderStatusIcon: TouchGrassIcon = createIcon(
  MoreHorizontalCircle01Icon,
  "ProviderStatusIcon",
);
const EllipsisIcon: TouchGrassIcon = createIcon(MoreHorizontalIcon, "EllipsisIcon");
const RefreshIcon: TouchGrassIcon = createIcon(HugeRefreshIcon, "RefreshIcon");
const SettingsIcon: TouchGrassIcon = createIcon(Settings01Icon, "SettingsIcon");
const RankingIcon: TouchGrassIcon = createIcon(HugeRankingIcon, "RankingIcon");
const InviteIcon: TouchGrassIcon = createIcon(UserAdd01Icon, "InviteIcon");

export {
  ArrowExpand01Icon,
  ArrowShrink02Icon,
  CheckIcon,
  DownloadIcon,
  EllipsisIcon,
  GripVerticalIcon,
  InviteIcon,
  RankingIcon,
  ProviderStatusIcon,
  RefreshIcon,
  SettingsIcon,
};
export type { TouchGrassIconProps };
