import type { AddTokenmaxxerStatusV1 } from "@touchgrass/contracts";

type AddTokenmaxxerFailure = Exclude<AddTokenmaxxerStatusV1, "added" | "already-added">;

const addTokenmaxxerFailureText = {
  invalid: "Use the format TG-ABC123.",
  "limit-reached": "You can add up to 100 Tokenmaxxers.",
  "not-found": "Tokenmaxxer not found.",
  self: "You cannot add your own TouchGrass ID.",
  unavailable: "Could not add the Tokenmaxxer. Try again.",
} satisfies Record<AddTokenmaxxerFailure, string>;

function createAddTokenmaxxerRequestGuard() {
  let activeRequest: number | null = null;
  let generation = 0;

  return {
    begin() {
      if (activeRequest !== null) return null;
      generation += 1;
      activeRequest = generation;
      return activeRequest;
    },
    finish(request: number) {
      if (activeRequest !== request) return false;
      activeRequest = null;
      return request === generation;
    },
    inFlight: () => activeRequest !== null,
    invalidate() {
      generation += 1;
    },
  };
}

function normalizeTouchGrassId(value: string) {
  return value.trim().replace(/^#/, "").toUpperCase();
}

function addTokenmaxxerHelpText({
  failure,
  touchGrassId,
  valid,
}: {
  failure: AddTokenmaxxerFailure | null;
  touchGrassId: string;
  valid: boolean;
}) {
  if (failure) return addTokenmaxxerFailureText[failure];
  return touchGrassId.length > 0 && !valid
    ? "Use the format TG-ABC123."
    : "Ask the Tokenmaxxer for their TouchGrass ID.";
}

export { addTokenmaxxerHelpText, createAddTokenmaxxerRequestGuard, normalizeTouchGrassId };
export type { AddTokenmaxxerFailure };
