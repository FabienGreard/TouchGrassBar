import type { StorybookConfig } from "@storybook/react-vite";
import tailwindcss from "@tailwindcss/vite";
import { mergeConfig } from "vite";

const config = {
  addons: ["@storybook/addon-docs"],
  docs: { defaultName: "Documentation" },
  framework: {
    name: "@storybook/react-vite",
    options: {},
  },
  stories: ["../src/**/*.mdx", "../src/**/*.stories.@(js|jsx|mjs|ts|tsx)"],
  viteFinal: async (config) =>
    mergeConfig(config, { plugins: [tailwindcss()] }),
} satisfies StorybookConfig;

export default config;
