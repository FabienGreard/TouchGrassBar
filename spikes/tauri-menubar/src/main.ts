import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./styles.css";

const currentWindow = getCurrentWindow();
const closeButton = document.querySelector<HTMLButtonElement>("#close-panel");
const title = document.querySelector<HTMLElement>("#window-title");
const copy = document.querySelector<HTMLElement>("#window-copy");

if (currentWindow.label === "settings") {
  document.body.dataset.window = "settings";
  if (title) title.textContent = "Settings window";
  if (copy) {
    copy.textContent =
      "A separate decorated window can coexist with the accessory menu-bar process.";
  }
  closeButton?.remove();
} else {
  closeButton?.addEventListener("click", () => invoke("hide_panel"));
  window.addEventListener("keydown", (event) => {
    if (event.key === "Escape") void invoke("hide_panel");
  });
}
