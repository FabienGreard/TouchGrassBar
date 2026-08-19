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

export { createAddTokenmaxxerRequestGuard, normalizeTouchGrassId };
