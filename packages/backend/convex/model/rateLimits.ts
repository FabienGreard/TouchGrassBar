import { MINUTE, RateLimiter } from "@convex-dev/rate-limiter";

import { components } from "../_generated/api";
import { backendPolicy } from "./policy";

export const touchGrassAuthPolicy = {
  failedRecoveryKey: {
    attempts: backendPolicy.recovery.failedAttempts,
    reservationMs: backendPolicy.recovery.failedAttemptReservationMs,
    windowMs: backendPolicy.recovery.failedAttemptWindowMs,
  },
  profilePreparation: {
    attempts: backendPolicy.authentication.profilePreparationAttempts,
    windowMs: backendPolicy.authentication.profilePreparationWindowMs,
  },
  successfulProfileRecovery: {
    attempts: backendPolicy.recovery.successfulTransfers,
    windowMs: backendPolicy.recovery.successfulTransferWindowMs,
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
    capacity: backendPolicy.synchronization.rateCapacity,
    kind: "token bucket",
    period: MINUTE,
    rate: backendPolicy.synchronization.ratePerMinute,
  },
  successfulProfileRecovery: {
    kind: "fixed window",
    period: touchGrassAuthPolicy.successfulProfileRecovery.windowMs,
    rate: touchGrassAuthPolicy.successfulProfileRecovery.attempts,
    start: 0,
  },
});
