import { MINUTE, RateLimiter } from "@convex-dev/rate-limiter";

import { components } from "../_generated/api";

export const touchGrassAuthPolicy = {
  failedRecoveryKey: {
    attempts: 5,
    reservationMs: 15 * MINUTE,
    windowMs: 15 * MINUTE,
  },
  profilePreparation: {
    attempts: 5,
    windowMs: MINUTE,
  },
  successfulProfileRecovery: {
    attempts: 3,
    windowMs: 60 * MINUTE,
  },
} as const;

export const rateLimiter = new RateLimiter(components.rateLimiter, {
  failedRecoveryKeyByIp: {
    kind: "fixed window",
    period: touchGrassAuthPolicy.failedRecoveryKey.windowMs,
    rate: touchGrassAuthPolicy.failedRecoveryKey.attempts,
    start: 0,
  },
  failedRecoveryKeyByTouchGrassId: {
    kind: "fixed window",
    period: touchGrassAuthPolicy.failedRecoveryKey.windowMs,
    rate: touchGrassAuthPolicy.failedRecoveryKey.attempts,
    start: 0,
  },
  profilePreparationByIp: {
    kind: "fixed window",
    period: touchGrassAuthPolicy.profilePreparation.windowMs,
    rate: touchGrassAuthPolicy.profilePreparation.attempts,
    start: 0,
  },
  syncDailyUsage: {
    capacity: 180,
    kind: "token bucket",
    period: MINUTE,
    rate: 60,
  },
  successfulProfileRecovery: {
    kind: "fixed window",
    period: touchGrassAuthPolicy.successfulProfileRecovery.windowMs,
    rate: touchGrassAuthPolicy.successfulProfileRecovery.attempts,
    start: 0,
  },
});
