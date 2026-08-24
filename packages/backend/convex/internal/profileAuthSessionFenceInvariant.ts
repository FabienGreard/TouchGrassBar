import type { PaginationResult } from "convex/server";
import { v } from "convex/values";

import { internal } from "../_generated/api";
import { type ActionCtx, internalAction } from "../_generated/server";

const PAGE_SIZE = 100;
const MAX_PAGES = 100;

type ProfileFenceState = {
  activeAuthSessionIdMissing: boolean;
  activeMacAuthorityInvalid: boolean;
  authSessionGenerationMissing: boolean;
};

const resultValidator = v.object({
  invalidActiveMacAuthorities: v.number(),
  missingActiveAuthSessionIds: v.number(),
  missingAuthSessionGenerations: v.number(),
  profiles: v.number(),
  profilesMissingFenceFields: v.number(),
});

export const check = internalAction({
  args: {},
  returns: resultValidator,
  handler: async (ctx: ActionCtx) => {
    let cursor: string | null = null;
    let isDone = false;
    let invalidActiveMacAuthorities = 0;
    let missingActiveAuthSessionIds = 0;
    let missingAuthSessionGenerations = 0;
    let profiles = 0;
    let profilesMissingFenceFields = 0;

    for (let pageNumber = 0; pageNumber < MAX_PAGES && !isDone; pageNumber += 1) {
      const page: PaginationResult<ProfileFenceState> = await ctx.runQuery(
        internal.internal.profileAuthSessionFenceInvariantPage.states,
        {
          paginationOpts: {
            cursor,
            maximumRowsRead: PAGE_SIZE,
            numItems: PAGE_SIZE,
          },
        },
      );
      for (const profile of page.page) {
        profiles += 1;
        if (profile.activeMacAuthorityInvalid) invalidActiveMacAuthorities += 1;
        if (profile.activeAuthSessionIdMissing) missingActiveAuthSessionIds += 1;
        if (profile.authSessionGenerationMissing) missingAuthSessionGenerations += 1;
        if (profile.activeAuthSessionIdMissing || profile.authSessionGenerationMissing) {
          profilesMissingFenceFields += 1;
        }
      }
      cursor = page.continueCursor;
      isDone = page.isDone;
    }
    if (!isDone) {
      throw new Error("Profile Auth Session fence check exceeded its bounded policy");
    }
    return {
      invalidActiveMacAuthorities,
      missingActiveAuthSessionIds,
      missingAuthSessionGenerations,
      profiles,
      profilesMissingFenceFields,
    };
  },
});
