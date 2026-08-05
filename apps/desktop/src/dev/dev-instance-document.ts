import type { DesktopSurface } from "@/App";
import {
  devAccentColors,
  type DevInstance,
} from "@/dev/dev-instance";

function applyDevInstanceDocument(
  instance: DevInstance,
  surface: DesktopSurface,
) {
  const surfaceName =
    surface === "panel"
      ? "TouchGrassBar"
      : `TouchGrassBar ${surface === "settings" ? "Settings" : "Onboarding"}`;
  document.title = `${surfaceName} · ${instance.label}`;
  document.documentElement.dataset.devInstance = instance.key;
  document.documentElement.style.setProperty(
    "--dev-instance-accent",
    devAccentColors[instance.accent],
  );
}

export { applyDevInstanceDocument };
