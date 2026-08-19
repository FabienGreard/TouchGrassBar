import type { Meta, StoryObj } from "@storybook/react-vite";

import { Button, ProviderConnectionCard } from "../index";

const meta = {
  component: ProviderConnectionCard,
  decorators: [
    (Story) => (
      <div className="w-[520px] max-w-full">
        <Story />
      </div>
    ),
  ],
  title: "Components/Coding Provider card",
} satisfies Meta<typeof ProviderConnectionCard>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Ready: Story = {
  args: {
    action: (
      <Button size="quiet" type="button" variant="ghost">
        Check now
      </Button>
    ),
    description: "Detected locally and reporting provider limits.",
    label: "Codex",
    provider: "codex",
    status: "Ready",
    statusTone: "ready",
  },
};

export const NeedsAccess: Story = {
  args: {
    action: (
      <Button size="quiet" type="button" variant="ghost">
        Check again
      </Button>
    ),
    description: "Claude is installed, but TouchGrassBar cannot read its local state yet.",
    label: "Claude",
    provider: "claude",
    status: "Needs access",
    statusTone: "attention",
  },
};

export const NotInstalled: Story = {
  args: {
    action: (
      <Button size="quiet" type="button" variant="ghost">
        Check again
      </Button>
    ),
    description: "Claude was not found in Applications or your command-line tools.",
    label: "Claude",
    provider: "claude",
    status: "Not installed",
    statusTone: "neutral",
  },
};
