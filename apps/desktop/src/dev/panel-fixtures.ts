import type { CurrentProfile, DoomerboardRow } from "@/components/panel/doomerboard";
import type { UsagePresentation } from "@/components/panel/usage-overview";

export const currentDoomerboardRows = [
  {
    displayName: "laura",
    note: "ABSOLUTELY FINE",
    rank: 1,
    tokenScore: "18.2M",
    touchGrassId: "#TG-4COLD7",
  },
  {
    displayName: "Fabien",
    note: "YOU",
    rank: 2,
    tokenScore: "12.8M",
    touchGrassId: "#TG-7K4P9D",
  },
  {
    displayName: "max",
    note: "STILL ONLINE",
    rank: 3,
    tokenScore: "9.1M",
    touchGrassId: "#TG-BURN42",
  },
  { displayName: "nora", rank: 4, tokenScore: "7.8M", touchGrassId: "#TG-NULL77" },
  { displayName: "eli", rank: 5, tokenScore: "6.4M", touchGrassId: "#TG-LOOP55" },
  { displayName: "mia", rank: 6, tokenScore: "4.9M", touchGrassId: "#TG-DIM420" },
  { displayName: "theo", rank: 7, tokenScore: "3.8M", touchGrassId: "#TG-GRASS7" },
  { displayName: "zara", rank: 8, tokenScore: "2.4M", touchGrassId: "#TG-SLEEP8" },
] as const satisfies readonly DoomerboardRow[];

export const myTokenmaxxerRows = [
  {
    displayName: "Fabien",
    note: "YOU",
    rank: 1,
    tokenScore: "12.8M",
    touchGrassId: "#TG-7K4P9D",
  },
  {
    displayName: "max",
    note: "TOUCH GRASS?",
    rank: 2,
    tokenScore: "9.1M",
    touchGrassId: "#TG-BURN42",
  },
  {
    displayName: "nora",
    note: "STILL ONLINE",
    rank: 3,
    tokenScore: "7.8M",
    touchGrassId: "#TG-NULL77",
  },
  { displayName: "eli", rank: 4, tokenScore: "6.4M", touchGrassId: "#TG-LOOP55" },
  { displayName: "mia", rank: 5, tokenScore: "4.9M", touchGrassId: "#TG-DIM420" },
] as const satisfies readonly DoomerboardRow[];

export const currentProfile = {
  displayName: "Fabien",
  touchGrassId: "#TG-7K4P9D",
} as const satisfies CurrentProfile;

// Development-only values reproduce the approved prototype and never enter a native snapshot.
export const currentUsagePresentation = {
  sevenDays: {
    gaugeFill: 64,
    trend: "+14%",
    trendDescription: "Up 14 percent from the previous 7 days",
  },
  thirtyDays: {
    gaugeFill: 100,
    trend: "+22%",
    trendDescription: "Up 22 percent from the previous 30 days",
  },
  today: {
    gaugeFill: 34,
    trend: "-8%",
    trendDescription: "Down 8 percent from the previous day",
  },
} as const satisfies UsagePresentation;
