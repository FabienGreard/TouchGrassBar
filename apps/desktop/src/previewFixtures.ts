export type DoomerboardPreviewRow = {
  id: string;
  name: string;
  note?: string;
  rank: number;
  score: string;
};

export type UsageMetricPreview = {
  gaugeFill: number;
  trend: string;
  trendDescription: string;
};

export type UsagePreview = {
  sevenDays: UsageMetricPreview;
  thirtyDays: UsageMetricPreview;
  today: UsageMetricPreview;
};

export const currentDoomerboardPreviewRows = [
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
] as const satisfies readonly DoomerboardPreviewRow[];

export const friendsDoomerboardPreviewRows = [
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
] as const satisfies readonly DoomerboardPreviewRow[];

// These values reproduce the approved prototype and are never added to a native snapshot.
export const currentUsagePreview = {
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
} as const satisfies UsagePreview;
