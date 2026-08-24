import type { PaginationOptions } from "convex/server";

export function assertBoundedProfilePagination(
  paginationOpts: PaginationOptions,
  maximumProfiles: number,
) {
  if (
    !Number.isSafeInteger(paginationOpts.numItems) ||
    paginationOpts.numItems < 1 ||
    paginationOpts.numItems > maximumProfiles
  ) {
    throw new Error(`Pagination numItems must be between 1 and ${maximumProfiles}`);
  }
  if (
    paginationOpts.maximumRowsRead === undefined ||
    !Number.isSafeInteger(paginationOpts.maximumRowsRead) ||
    paginationOpts.maximumRowsRead < 1 ||
    paginationOpts.maximumRowsRead > maximumProfiles
  ) {
    throw new Error(`Pagination maximumRowsRead must be between 1 and ${maximumProfiles}`);
  }
}
