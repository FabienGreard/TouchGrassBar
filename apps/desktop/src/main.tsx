import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "@/App";
import {
  createBrowserSanitizedDesktopStateAdapter,
  resolveBrowserFixtureName,
} from "@/browserSanitizedDesktopStateAdapter";
import { createSanitizedDesktopStateDelivery } from "@/sanitizedDesktopStateDelivery";
import "@/styles.css";
import { createTauriSanitizedDesktopStateAdapter } from "@/tauriSanitizedDesktopStateAdapter";

const root = document.getElementById("root");
if (!root) {
  throw new Error("TouchGrassBar root element is missing");
}

const hasNativeRuntime = "__TAURI_INTERNALS__" in window;
const hasBrowserPreview = import.meta.env.DEV && !hasNativeRuntime;
const previewFixtureName = hasBrowserPreview
  ? resolveBrowserFixtureName(window.location.search)
  : "unavailable";
const stateDelivery = createSanitizedDesktopStateDelivery(
  hasNativeRuntime
    ? createTauriSanitizedDesktopStateAdapter()
    : createBrowserSanitizedDesktopStateAdapter(previewFixtureName),
);

if (hasBrowserPreview) {
  document.documentElement.dataset.desktopPreview = "true";
}

createRoot(root).render(
  <StrictMode>
    <App
      hasNativeRuntime={hasNativeRuntime}
      previewFixtureName={previewFixtureName}
      stateDelivery={stateDelivery}
    />
  </StrictMode>,
);
