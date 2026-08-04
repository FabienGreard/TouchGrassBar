import type { Meta, StoryObj } from "@storybook/react-vite";

import { ProviderMark } from "../index";

const meta = {
  args: { alt: "", provider: "codex", size: "default" },
  argTypes: {
    alt: { control: "text" },
    className: { table: { disable: true } },
    provider: {
      control: "inline-radio",
      options: ["codex", "claude"],
    },
    size: {
      control: "inline-radio",
      options: ["default", "large"],
    },
  },
  component: ProviderMark,
  parameters: {
    docs: {
      description: {
        component:
          "The exact provider marks used in the desktop panel and native setup screens.",
      },
    },
  },
  title: "Foundation/Provider marks",
} satisfies Meta<typeof ProviderMark>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Codex: Story = {};

export const Claude: Story = {
  args: { provider: "claude" },
};
