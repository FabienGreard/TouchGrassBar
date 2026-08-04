import { BrandMark, NativeWindowSidebar } from "@touchgrass/ui";
import type { ReactNode } from "react";

type ScreenSidebarProps = {
  children: ReactNode;
  footer: string;
  title: string;
};

function ScreenSidebar({ children, footer, title }: ScreenSidebarProps) {
  return (
    <NativeWindowSidebar>
      <span className="mx-2 grid h-9 w-9 place-items-center rounded-[9px] border border-white/30 bg-action shadow-action">
        <BrandMark size="sidebar" tone="reversed" />
      </span>
      <strong className="mx-2 mt-3 text-[16px] tracking-[-0.025em]">
        {title}
      </strong>
      {children}
      <small className="mx-2 mt-auto text-[10px] leading-4 text-sheet-muted">
        {footer}
      </small>
    </NativeWindowSidebar>
  );
}

export { ScreenSidebar };
