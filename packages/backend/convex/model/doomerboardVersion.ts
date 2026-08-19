import type { MutationCtx, QueryCtx } from "../_generated/server";

async function versionRow(ctx: Pick<QueryCtx, "db">) {
  const rows = await ctx.db.query("doomerboardVersions").take(2);
  if (rows.length > 1) {
    throw new Error("Doomerboard version singleton invariant failed");
  }
  const row = rows[0] ?? null;
  if (row && (!Number.isSafeInteger(row.version) || row.version < 0)) {
    throw new Error("Doomerboard version is invalid");
  }
  return row;
}

export async function readDoomerboardVersion(ctx: Pick<QueryCtx, "db">) {
  return (await versionRow(ctx))?.version ?? 0;
}

export async function markDoomerboardChanged(ctx: MutationCtx) {
  const row = await versionRow(ctx);
  if (!row) {
    await ctx.db.insert("doomerboardVersions", { version: 1 });
    return;
  }
  if (row.version >= Number.MAX_SAFE_INTEGER) {
    throw new Error("Doomerboard version exceeds the safe integer range");
  }
  await ctx.db.patch(row._id, { version: row.version + 1 });
}
