import { defineConfig } from "vitest/config";

export default defineConfig({
  server: {
    deps: {
      inline: ["convex-test"],
    },
  },
  test: {
    environment: "edge-runtime",
  },
});
