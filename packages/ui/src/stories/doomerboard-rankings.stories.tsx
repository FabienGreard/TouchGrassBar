import type { Meta, StoryObj } from "@storybook/react-vite";

import { DoomerboardRankings } from "../index";

const rows = [
  {
    displayName: "laura",
    note: "ABSOLUTELY FINE",
    rank: 1,
    tokenScore: "18.2M",
    touchGrassId: "TG-GOLD01",
  },
  { displayName: "you", note: "YOU", rank: 2, tokenScore: "12.8M", touchGrassId: "TG-GRASS2" },
  {
    displayName: "max",
    note: "STILL ONLINE",
    rank: 3,
    tokenScore: "9.1M",
    touchGrassId: "TG-BURN42",
  },
  { displayName: "nora", rank: 4, tokenScore: "7.8M", touchGrassId: "TG-NULL77" },
] as const;

const meta = {
  component: DoomerboardRankings,
  parameters: {
    docs: {
      description: {
        component:
          "The shared business-stateless podium and ledger presentation for Doomerboard rows.",
      },
    },
  },
  title: "Product/Doomerboard rankings",
} satisfies Meta<typeof DoomerboardRankings>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: { rows },
  decorators: [
    (Story) => (
      <div className="h-[280px] w-[402px] max-w-full overflow-hidden bg-panel-glass text-pearl-ink">
        <Story />
      </div>
    ),
  ],
};
