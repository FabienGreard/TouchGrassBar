import type { Meta, StoryObj } from "@storybook/react-vite";

import { BrandMark } from "../index";

const meta = {
  args: {
    alt: "",
    size: "panel",
    tone: "ink",
  },
  argTypes: {
    alt: { control: "text" },
    className: { table: { disable: true } },
    size: {
      control: "inline-radio",
      options: ["panel", "sidebar"],
    },
    tone: {
      control: "inline-radio",
      options: ["ink", "reversed"],
    },
  },
  component: BrandMark,
  parameters: {
    docs: {
      description: {
        component:
          "The approved lily glyph at the two sizes and contrast treatments used by the panel and native sidebar.",
      },
    },
  },
  title: "Foundation/Brand mark",
} satisfies Meta<typeof BrandMark>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Mark: Story = {};
