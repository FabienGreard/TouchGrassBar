import { DirectAggregate } from "@convex-dev/aggregate";
import type { GenericId } from "convex/values";

import { components } from "../_generated/api";

export type DoomerboardKey = [number, string];

export const doomerboard = new DirectAggregate<{
  Id: GenericId<"publicUsages">;
  Key: DoomerboardKey;
  Namespace: string;
}>(components.doomerboard);

export function doomerboardKey(tokenScore: number, touchGrassId: string): DoomerboardKey {
  return [-tokenScore, touchGrassId];
}
