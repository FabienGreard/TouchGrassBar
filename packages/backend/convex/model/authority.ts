import { ConvexError } from "convex/values";

export const AUTHORITY_REJECTED_CODE = "authority-rejected" as const;

export function rejectAuthority(): never {
  throw new ConvexError({ code: AUTHORITY_REJECTED_CODE });
}
