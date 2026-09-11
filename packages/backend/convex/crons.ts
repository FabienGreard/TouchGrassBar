import { cronJobs } from "convex/server";

import { internal } from "./_generated/api";

const crons = cronJobs();

crons.interval(
  "delete expired failure reports",
  { hours: 1 },
  internal.diagnostics.deleteExpired,
  {},
);

crons.cron(
  "recompute every Profile rolling score",
  "5 0 * * *",
  internal.internal.recompute.scheduleAll,
  {},
);

crons.interval(
  "monitor daily score recomputation",
  { minutes: 10 },
  internal.internal.recompute.monitor,
  {},
);

export default crons;
