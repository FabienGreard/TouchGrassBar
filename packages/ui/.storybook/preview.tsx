import type { Preview } from "@storybook/react-vite";

import "../src/theme.css";

const preview = {
  parameters: {
    controls: {
      expanded: true,
      matchers: {
        color: /(background|color)$/i,
        date: /Date$/i,
      },
    },
    options: {
      storySort: {
        order: ["Foundation", "Primitives", "Components", "Surfaces"],
      },
    },
  },
  tags: ["autodocs"],
} satisfies Preview;

export default preview;
