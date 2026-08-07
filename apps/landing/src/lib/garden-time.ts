export const gardenTimes = ["dawn", "day", "golden", "night"] as const;

export type GardenTime = (typeof gardenTimes)[number];

export function gardenTimeForHour(hour: number): GardenTime {
  if (!Number.isInteger(hour) || hour < 0 || hour > 23) {
    throw new Error("The local hour must be an integer from 0 through 23.");
  }
  if (hour >= 5 && hour < 9) return "dawn";
  if (hour >= 9 && hour < 17) return "day";
  if (hour >= 17 && hour < 21) return "golden";
  return "night";
}

export function applyGardenTime(documentObject: Document, date = new Date()) {
  const gardenTime = gardenTimeForHour(date.getHours());
  documentObject.documentElement.dataset.gardenTime = gardenTime;
  for (const label of documentObject.querySelectorAll<HTMLElement>(
    "[data-garden-time-label]",
  )) {
    label.textContent = gardenTime;
  }
  return gardenTime;
}

export function installGardenTime(documentObject: Document) {
  applyGardenTime(documentObject);
  const timer = window.setInterval(
    () => applyGardenTime(documentObject),
    60_000,
  );
  return () => window.clearInterval(timer);
}
