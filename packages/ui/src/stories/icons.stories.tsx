import type { Meta, StoryObj } from "@storybook/react-vite";

import {
  CheckIcon,
  DownloadIcon,
  EllipsisIcon,
  InviteIcon,
  RankingIcon,
  ProviderStatusIcon,
  RefreshIcon,
  SettingsIcon,
} from "../index";

const meta = {
  args: {
    size: 24,
    spin: false,
    strokeWidth: 1.7,
    tone: "default",
  },
  argTypes: {
    size: {
      control: { max: 48, min: 12, step: 1, type: "range" },
    },
    spin: {
      control: "boolean",
      description: "Preview the refresh motion treatment.",
    },
    strokeWidth: {
      control: { max: 2.5, min: 1, step: 0.1, type: "range" },
      description: "Adjust the stroke width across the complete icon catalog.",
    },
    tone: {
      control: "inline-radio",
      options: ["default", "muted", "primary", "unavailable"],
    },
  },
  component: RefreshIcon,
  parameters: {
    docs: {
      description: {
        component:
          "The Hugeicons free stroke-rounded icons consumed by TouchGrassBar. Every product icon flows through the shared adapter and inherits the default black ink color.",
      },
    },
  },
  title: "Foundation/Icons",
} satisfies Meta<typeof RefreshIcon>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Catalog: Story = {
  render: ({ size = 24, spin = false, strokeWidth = 1.7, tone = "default" }) => (
    <div className="grid grid-cols-2 gap-5 sm:grid-cols-4">
      <span className="flex items-center gap-2 text-xs">
        <RefreshIcon
          aria-label="Refresh"
          size={size}
          spin={spin}
          strokeWidth={strokeWidth}
          tone={tone}
        />
        Refresh
      </span>
      <span className="flex items-center gap-2 text-xs">
        <DownloadIcon
          aria-label="Download update"
          size={size}
          strokeWidth={strokeWidth}
          tone={tone}
        />
        Download update
      </span>
      <span className="flex items-center gap-2 text-xs">
        <InviteIcon aria-label="Invite" size={size} strokeWidth={strokeWidth} tone={tone} />
        Invite
      </span>
      <span className="flex items-center gap-2 text-xs">
        <SettingsIcon aria-label="Settings" size={size} strokeWidth={strokeWidth} tone={tone} />
        Settings
      </span>
      <span className="flex items-center gap-2 text-xs">
        <RankingIcon aria-label="Doomerboard" size={size} strokeWidth={strokeWidth} tone={tone} />
        Doomerboard
      </span>
      <span className="flex items-center gap-2 text-xs">
        <ProviderStatusIcon
          aria-label="Provider status"
          size={size}
          strokeWidth={strokeWidth}
          tone={tone}
        />
        Provider
      </span>
      <span className="flex items-center gap-2 text-xs">
        <EllipsisIcon aria-label="More" size={size} strokeWidth={strokeWidth} tone={tone} />
        More
      </span>
      <span className="flex items-center gap-2 text-xs">
        <CheckIcon aria-label="Selected" size={size} strokeWidth={strokeWidth} tone={tone} />
        Selected
      </span>
    </div>
  ),
};
