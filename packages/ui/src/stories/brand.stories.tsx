import type { Meta, StoryObj } from "@storybook/react-vite";

import { Brand, BrandWordmark } from "../index";

const meta = {
  component: Brand,
  parameters: {
    docs: {
      description: {
        component:
          "The approved TouchGrassBar identity and its canonical palette. The joined wordmark, actions, and focus outlines share one luminous lime primary family.",
      },
    },
  },
  title: "Foundation/Brand",
} satisfies Meta<typeof Brand>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Panel: Story = {};

export const Wordmark: Story = {
  render: () => <BrandWordmark />,
};
