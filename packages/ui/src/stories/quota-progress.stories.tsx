import type { Meta, StoryObj } from "@storybook/react-vite";

import { QuotaProgress } from "../index";

const meta = {
  args: {
    "aria-label": "Codex quota 74 percent remaining",
    provider: "codex",
    size: "primary",
    value: 74,
  },
  argTypes: {
    "aria-label": { control: "text" },
    className: { table: { disable: true } },
    provider: {
      control: "inline-radio",
      options: ["codex", "claude"],
    },
    size: {
      control: "inline-radio",
      options: ["primary", "secondary"],
    },
    value: {
      control: { max: 100, min: 0, step: 1, type: "range" },
    },
  },
  component: QuotaProgress,
  decorators: [
    (Story) => (
      <div className="w-[320px]">
        <Story />
      </div>
    ),
  ],
  parameters: {
    docs: {
      description: {
        component:
          "The shipped provider quota gauge. Its value changes shade within Codex blue or Claude orange without crossing into another status hue.",
      },
    },
  },
  title: "Primitives/Quota progress",
} satisfies Meta<typeof QuotaProgress>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Codex: Story = {};

export const Claude: Story = {
  args: {
    "aria-label": "Claude quota 18 percent remaining",
    provider: "claude",
    value: 18,
  },
};

export const Unavailable: Story = {
  args: {
    "aria-label": "Quota unavailable",
    value: null,
  },
};
