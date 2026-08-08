const gardenTimes = ["dawn", "day", "golden", "night"] as const;

export type GardenTime = (typeof gardenTimes)[number];

export const GARDEN_COPY: Record<GardenTime, readonly [string, string, string]> = {
  dawn: ["The sun is up.", "Apparently,", "So are you."],
  day: ["It's nice outside.", "According to", "Reliable sources."],
  golden: ["Even the sun", "Has logged off.", "You have not."],
  night: ["Nothing good", "Gets deployed", "At this hour."],
};

export function isGardenTime(value: string | null | undefined): value is GardenTime {
  return (gardenTimes as readonly string[]).includes(value ?? "");
}

export function gardenTimeForHour(hour: number): GardenTime {
  if (!Number.isInteger(hour) || hour < 0 || hour > 23) {
    throw new Error("The local hour must be an integer from 0 through 23.");
  }
  if (hour >= 5 && hour < 9) return "dawn";
  if (hour >= 9 && hour < 17) return "day";
  if (hour >= 17 && hour < 21) return "golden";
  return "night";
}
