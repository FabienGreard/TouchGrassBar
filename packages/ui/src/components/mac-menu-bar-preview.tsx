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
          <span
            aria-hidden="true"
            className="relative h-6 w-[26.7px]"
            data-meter-shape="rounded-pill"
            data-native-meter-geometry="332x48"
            data-preview-value="illustrative"
            data-slot="menu-bar-headroom-icon"
          >
            <BrandMark
              className="absolute top-0 left-1/2 size-[19.2px] -translate-x-1/2"
              tone="reversed"
            />
            <span
              className="absolute bottom-[0.6px] left-[0.9px] h-[3.6px] w-[24.9px] overflow-hidden rounded-full bg-white/35"
              data-slot="menu-bar-headroom-meter"
            >
              <span
                className="block h-full w-3/5 rounded-full bg-white"
                data-slot="menu-bar-headroom-fill"
              />
            </span>
          </span>
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
        <span className="flex items-center gap-2 text-[10px] font-medium tracking-[-0.015em] whitespace-nowrap text-white">
          <span>{dateLabel}</span>
          <span>{timeLabel}</span>
        </span>
      </div>
    </div>
  );
}

export { MacMenuBarPreview };
export type { MacMenuBarPreviewProps };
