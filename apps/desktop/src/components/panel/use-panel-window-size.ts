import { invoke } from "@tauri-apps/api/core";
import { useEffect } from "react";

function usePanelWindowSize(enabled: boolean) {
  useEffect(() => {
    if (!enabled || typeof ResizeObserver === "undefined") return;

    const panel = document.querySelector<HTMLElement>('[data-slot="panel-shell"]');
    if (!panel) return;

    let lastHeight = 0;
    const observer = new ResizeObserver(() => {
      const height = Math.ceil(panel.getBoundingClientRect().height);
      if (height === lastHeight) return;
      lastHeight = height;
      void invoke("resize_panel", { height });
    });
    observer.observe(panel);

    return () => observer.disconnect();
  }, [enabled]);
}

export { usePanelWindowSize };
