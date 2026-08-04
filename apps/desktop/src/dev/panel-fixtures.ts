import type {
  CurrentProfile,
  DoomerboardRow,
} from "@/components/panel/doomerboard";
import type { UsagePresentation } from "@/components/panel/usage-overview";

export const currentDoomerboardRows = [
  {
    id: "#TG-4COLD7",
    name: "laura",
    note: "ABSOLUTELY FINE",
    rank: 1,
    score: "18.2M",
  },
  { id: "#TG-7K4P9D", name: "Fabien", note: "YOU", rank: 2, score: "12.8M" },
  {
    id: "#TG-BURN42",
    name: "max",
    note: "STILL ONLINE",
    rank: 3,
    score: "9.1M",
  },
  { id: "#TG-NULL77", name: "nora", rank: 4, score: "7.8M" },
  { id: "#TG-LOOP55", name: "eli", rank: 5, score: "6.4M" },
  { id: "#TG-DIM420", name: "mia", rank: 6, score: "4.9M" },
  { id: "#TG-GRASS7", name: "theo", rank: 7, score: "3.8M" },
  { id: "#TG-SLEEP8", name: "zara", rank: 8, score: "2.4M" },
] as const satisfies readonly DoomerboardRow[];

export const myTokenmaxxerRows = [
  { id: "#TG-7K4P9D", name: "Fabien", note: "YOU", rank: 1, score: "12.8M" },
  {
    id: "#TG-BURN42",
    name: "max",
    note: "TOUCH GRASS?",
    rank: 2,
    score: "9.1M",
  },
  {
    id: "#TG-NULL77",
    name: "nora",
    note: "STILL ONLINE",
    rank: 3,
    score: "7.8M",
  },
  { id: "#TG-LOOP55", name: "eli", rank: 4, score: "6.4M" },
  { id: "#TG-DIM420", name: "mia", rank: 5, score: "4.9M" },
] as const satisfies readonly DoomerboardRow[];

export const currentProfile = {
  id: "#TG-7K4P9D",
  name: "Fabien",
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
