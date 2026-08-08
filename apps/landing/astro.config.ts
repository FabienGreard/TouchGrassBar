import react from "@astrojs/react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "astro/config";

export default defineConfig({
  integrations: [react()],
  output: "static",
  site: "https://touchgrassbar.com",
  vite: {
    plugins: [tailwindcss()],
  },
});
