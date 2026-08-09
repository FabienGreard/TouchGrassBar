import { DirectAggregate } from "@convex-dev/aggregate";
import type { GenericId } from "convex/values";

import { components } from "../_generated/api";

export const globalDoomerboardIndex = new DirectAggregate<{
  Id: GenericId<"publicScores">;
  Key: number;
  Namespace: string;
}>(components.doomerboard);
