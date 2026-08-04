import { getCurrentWindow } from "@tauri-apps/api/window";
import { StrictMode, type ReactNode } from "react";
import { createRoot } from "react-dom/client";

import { App, type DesktopSurface } from "@/App";
import { createSanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";
import "@/styles.css";
import { createTauriSanitizedDesktopStateAdapter } from "@/native-state/tauri-sanitized-desktop-state-adapter";

const rootElement = document.getElementById("root");
if (!rootElement) {
  throw new Error("TouchGrassBar root element is missing");
}

const root = createRoot(rootElement);
const hasNativeRuntime = "__TAURI_INTERNALS__" in window;

if (hasNativeRuntime) {
  document.documentElement.dataset.nativeRuntime = "true";
}

function render(application: ReactNode) {
  root.render(<StrictMode>{application}</StrictMode>);
}

if (hasNativeRuntime) {
  const label = getCurrentWindow().label;
  const surface: DesktopSurface =
    label === "settings" || label === "onboarding" ? label : "panel";

  if (surface === "panel") {
    document.documentElement.dataset.nativePanel = "true";
    const stateDelivery = createSanitizedDesktopStateDelivery(
      createTauriSanitizedDesktopStateAdapter(),
    );
    render(
      <App hasNativeRuntime stateDelivery={stateDelivery} surface="panel" />,
    );
  } else {
    render(<App hasNativeRuntime surface={surface} />);
  }
} else if (import.meta.env.DEV) {
  void import("@/dev/dev-preview-app").then(({ DevPreviewApp }) => {
    render(<DevPreviewApp />);
  });
} else {
  throw new Error("TouchGrassBar requires its native runtime");
}
