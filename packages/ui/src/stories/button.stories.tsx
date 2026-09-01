import type { Meta, StoryObj } from "@storybook/react-vite";

import { Button } from "../index";

const meta = {
  args: {
    children: "Global",
    disabled: false,
    size: "default",
    variant: "primary",
  },
  argTypes: {
    asChild: { table: { disable: true } },
    children: { control: "text" },
    className: { table: { disable: true } },
    disabled: { control: "boolean" },
    size: {
      control: "select",
      options: ["default", "quiet", "link", "icon", "sheet"],
    },
    variant: {
      control: "select",
      options: ["primary", "secondary", "ghost", "link"],
    },
  },
  component: Button,
  parameters: {
    docs: {
      description: {
        component:
          "The shared shadcn-based button. Primary, secondary, ghost, and link describe visual hierarchy; disabled is an interaction state.",
      },
    },
  },
  title: "Primitives/Button",
} satisfies Meta<typeof Button>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Primary: Story = {};

export const Secondary: Story = {
  args: {
    children: "Cancel",
    variant: "secondary",
  },
};

export const Ghost: Story = {
  args: {
    children: "Friends",
    size: "quiet",
    variant: "ghost",
  },
};

export const Link: Story = {
  args: {
    children: "View installation steps",
    size: "link",
    variant: "link",
  },
};

export const Disabled: Story = {
  args: {
    children: "Add item",
    disabled: true,
  },
};
