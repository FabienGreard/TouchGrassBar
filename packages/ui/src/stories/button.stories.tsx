import type { Meta, StoryObj } from "@storybook/react-vite";

import { Button } from "../index";

const meta = {
  args: {
    children: "Global",
    disabled: false,
    size: "default",
    variant: "action",
  },
  argTypes: {
    asChild: { table: { disable: true } },
    children: { control: "text" },
    className: { table: { disable: true } },
    disabled: { control: "boolean" },
    size: {
      control: "select",
      options: ["default", "identity", "quiet", "icon", "sheet"],
    },
    variant: {
      control: "select",
      options: ["action", "quiet"],
    },
  },
  component: Button,
  parameters: {
    docs: {
      description: {
        component:
          "The shared shadcn-based button used by actions, quiet triggers, identity copy, and the panel menu. Shipped visual treatments are named variants and sizes rather than desktop overrides.",
      },
    },
  },
  title: "Primitives/Button",
} satisfies Meta<typeof Button>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Action: Story = {};

export const Quiet: Story = {
  args: {
    children: "Tokenmaxxers",
    size: "quiet",
    variant: "quiet",
  },
};

export const DisabledInvite: Story = {
  args: {
    children: "Invite a friend",
    disabled: true,
  },
};

export const Identity: Story = {
  args: {
    children: "Fabien#TG-7K4P9D",
    size: "identity",
    variant: "quiet",
  },
};
