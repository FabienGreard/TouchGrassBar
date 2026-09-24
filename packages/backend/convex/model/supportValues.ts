import { v } from "convex/values";
import { supportReportValidator } from "./diagnosticValues";

export const supportSessionFields = {
  tokenmaxxerId: v.id("tokenmaxxers"),
  deviceId: v.id("devices"),
  generation: v.number(),
  createdAt: v.number(),
  expiresAt: v.number(),
  cancelledAt: v.union(v.number(), v.null()),
  deleteAt: v.number(),
};
export const supportRequestFields = {
  sessionId: v.id("supportSessions"),
  requestedAt: v.number(),
  deadline: v.number(),
  operator: v.string(),
  completedAt: v.union(v.number(), v.null()),
  report: v.union(supportReportValidator, v.null()),
  failure: v.union(v.literal("report_unavailable"), v.null()),
  deleteAt: v.number(),
};
export const supportSessionValidator = v.object({
  _id: v.id("supportSessions"),
  _creationTime: v.number(),
  ...supportSessionFields,
});
export const supportRequestValidator = v.object({
  _id: v.id("supportRequests"),
  _creationTime: v.number(),
  ...supportRequestFields,
});
