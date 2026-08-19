import { touchGrassIdSchema, type AddTokenmaxxerOutcome } from "@touchgrass/contracts";

type AddTokenmaxxerDialogStatus = AddTokenmaxxerOutcome["status"] | "idle" | "submitting";

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

function validTouchGrassId(value: string) {
  return touchGrassIdSchema.safeParse(value).success;
}

function addTokenmaxxerHelpText(status: AddTokenmaxxerDialogStatus) {
  switch (status) {
    case "added":
      return "Tokenmaxxer added.";
    case "already-added":
      return "Already in My Tokenmaxxers.";
    case "invalid":
      return "Use the format TG-ABC234.";
    case "limit-reached":
      return "My Tokenmaxxers is limited to 100.";
    case "not-found":
      return "No Tokenmaxxer has that TouchGrass ID.";
    case "self":
      return "That is your TouchGrass ID.";
    case "submitting":
      return "Adding Tokenmaxxer…";
    case "unavailable":
      return "Adding a Tokenmaxxer is unavailable. Try again.";
    case "idle":
      return "Ask the Tokenmaxxer for their TouchGrass ID.";
  }
}

export {
  addTokenmaxxerHelpText,
  createAddTokenmaxxerRequestGuard,
  normalizeTouchGrassId,
  validTouchGrassId,
};
export type { AddTokenmaxxerDialogStatus };
