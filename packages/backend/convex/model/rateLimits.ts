import { MINUTE, RateLimiter } from "@convex-dev/rate-limiter";

import { components } from "../_generated/api";

export const rateLimiter = new RateLimiter(components.rateLimiter, {
  syncDailyUsage: {
    capacity: 180,
    kind: "token bucket",
    period: MINUTE,
    rate: 60,
  },
});
