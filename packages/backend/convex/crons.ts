import { cronJobs } from "convex/server";

import { internal } from "./_generated/api";

const crons = cronJobs();

crons.daily(
  "recompute recently active rolling scores",
  { hourUTC: 0, minuteUTC: 5 },
  internal.internal.recompute.scheduleRecentlyActive,
  {},
);

export default crons;
