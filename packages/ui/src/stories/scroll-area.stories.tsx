import type { Meta, StoryObj } from "@storybook/react-vite";

import { ScrollArea } from "../index";

const ranks = Array.from({ length: 12 }, (_, index) => ({
  id: `#TG-STORY${String(index + 1).padStart(2, "0")}`,
  name: `Tokenmaxxer ${index + 1}`,
  rank: index + 1,
}));

const meta = {
  component: ScrollArea,
  parameters: {
    docs: {
      description: {
        component:
          "The shared shadcn/Radix scroll area used for bounded native surfaces such as the Doomerboard rankings.",
      },
    },
    layout: "centered",
  },
  title: "Primitives/Scroll area",
} satisfies Meta<typeof ScrollArea>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Rankings: Story = {
  render: () => (
    <ScrollArea
      aria-label="Example rankings"
      className="h-[220px] w-[360px] rounded-[12px] border border-pearl-line bg-pearl-surface"
      viewportClassName="select-none"
    >
      <div className="divide-y divide-pearl-line px-3">
        {ranks.map((row) => (
          <div
            className="grid grid-cols-[32px_1fr_auto] items-center py-3 text-[11px]"
            key={row.id}
          >
            <strong>{row.rank}</strong>
            <span>{row.name}</span>
            <small className="font-mono text-[8px] text-pearl-muted">{row.id}</small>
          </div>
        ))}
      </div>
    </ScrollArea>
  ),
};
