import type { ComponentProps } from "react";

import macosBatteryIcon from "../assets/macos-menu-bar/battery-charging.png";
import macosCursorIcon from "../assets/macos-menu-bar/cursor-arrow.png";
import macosSearchIcon from "../assets/macos-menu-bar/search.png";
import macosWifiIcon from "../assets/macos-menu-bar/wifi.png";
import { BrandMark } from "./brand";
import { cn } from "../lib/utils";

type MacMenuBarPreviewProps = ComponentProps<"div"> & {
  dateLabel?: string;
  timeLabel?: string;
};

function MacMenuBarPreview({
  className,
  dateLabel = "Tue 4 Aug",
  timeLabel = "17:15",
  ...props
}: MacMenuBarPreviewProps) {
  return (
    <div
      aria-label="TouchGrassBar menu bar preview"
      className={cn(
        "overflow-visible rounded-[6px] border border-black/30 bg-[linear-gradient(180deg,#414437_0%,#34372d_100%)] shadow-[inset_0_1px_0_rgba(255,255,255,0.08),0_2px_5px_rgba(20,22,18,0.2)]",
        className,
      )}
      data-slot="mac-menu-bar-preview"
      {...props}
    >
      <div className="flex h-8 items-center justify-end gap-2.5 px-2.5 text-white">
        <span className="menu-bar-app-target relative grid size-7 shrink-0 place-items-center rounded-[5px]">
          <BrandMark className="size-[19px]" tone="reversed" />
          <img
            alt=""
            className="menu-bar-cursor-click absolute top-[15px] left-[15px] z-10 h-[23px] w-[17px]"
            data-icon-source="macos-native-cursor-arrow"
            src={macosCursorIcon}
          />
        </span>
        <img
          alt=""
          className="h-[18px] w-[26px] object-contain"
          data-icon-source="macos-sf-battery-charging"
          src={macosBatteryIcon}
        />
        <img
          alt=""
          className="size-[17px] object-contain"
          data-icon-source="macos-sf-wifi"
          src={macosWifiIcon}
        />
        <img
          alt=""
          className="size-[15px] object-contain"
          data-icon-source="macos-sf-search"
          src={macosSearchIcon}
        />
        <span className="flex items-center gap-2 whitespace-nowrap text-[10px] font-medium tracking-[-0.015em] text-white">
          <span>{dateLabel}</span>
          <span>{timeLabel}</span>
        </span>
      </div>
    </div>
  );
}

export { MacMenuBarPreview };
export type { MacMenuBarPreviewProps };
