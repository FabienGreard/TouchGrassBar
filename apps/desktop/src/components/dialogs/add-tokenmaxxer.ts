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
  notFound,
  touchGrassId,
  valid,
}: {
  notFound: boolean;
  touchGrassId: string;
  valid: boolean;
}) {
  if (notFound) return "Tokenmaxxer not found.";
  return touchGrassId.length > 0 && !valid
    ? "Use the format TG-ABC123."
    : "Ask the Tokenmaxxer for their TouchGrass ID.";
}

export { addTokenmaxxerHelpText, createAddTokenmaxxerRequestGuard, normalizeTouchGrassId };
