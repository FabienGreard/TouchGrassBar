import { DirectAggregate } from "@convex-dev/aggregate";
import type { GenericId } from "convex/values";
import { v } from "convex/values";

import { components } from "../_generated/api";

export type LegacyDoomerboardKey = number;
export type DoomerboardKey = [number, string];
export type StoredDoomerboardKey =
  | LegacyDoomerboardKey
  | DoomerboardKey;

export const storedDoomerboardKeyValidator = v.union(
  v.number(),
  v.array(v.union(v.number(), v.string())),
);

export function isStoredDoomerboardKey(
  key: number | (number | string)[],
): key is StoredDoomerboardKey {
  return (
    typeof key === "number" ||
    (key.length === 2 &&
      typeof key[0] === "number" &&
      typeof key[1] === "string")
  );
}

export const doomerboard = new DirectAggregate<{
  Id: GenericId<"publicUsages">;
  Key: StoredDoomerboardKey;
  Namespace: string;
}>(components.doomerboard);

export function doomerboardKey(
  tokenScore: number,
  touchGrassId: string,
): DoomerboardKey {
  return [-tokenScore, touchGrassId];
}
