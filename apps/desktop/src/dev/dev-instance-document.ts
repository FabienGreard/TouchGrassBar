import type { DesktopSurface } from "@/App";
import {
  devAccentColors,
  type DevInstance,
} from "@/dev/dev-instance";
import "@/dev/dev-instance.css";

function applyDevInstanceDocument(
  instance: DevInstance,
  surface: DesktopSurface,
) {
  const surfaceName =
    surface === "panel"
      ? "TouchGrassBar"
      : `TouchGrassBar ${surface === "settings" ? "Settings" : "Onboarding"}`;
  document.title = `${surfaceName} · ${instance.label}`;
  document.documentElement.dataset.devInstance = instance.instanceKey;
  document.documentElement.style.setProperty(
    "--dev-instance-accent",
    devAccentColors[instance.accent],
  );
}

export { applyDevInstanceDocument };
