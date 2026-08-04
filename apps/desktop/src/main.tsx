import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "@/App";
import "@/styles.css";

const root = document.getElementById("root");
if (!root) {
  throw new Error("TouchGrassBar root element is missing");
}

if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  document.documentElement.dataset.desktopPreview = "true";
}

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
