import type { Meta, StoryObj } from "@storybook/react-vite";

import { NativeSheetSurface } from "../index";

const meta = {
  component: NativeSheetSurface,
  parameters: {
    docs: {
      description: {
        component:
          "The shared ivory native sheet surface used by static cards and interactive dialogs.",
      },
    },
  },
  title: "Surfaces/Native sheet",
} satisfies Meta<typeof NativeSheetSurface>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: () => (
    <NativeSheetSurface className="w-[340px] max-w-full p-4">
      <h2 className="m-0 text-[14px] font-bold">Native sheet</h2>
      <p className="mt-1 mb-0 text-[10px] leading-4 text-pearl-muted">
        Shared surface material for product cards and dialogs.
      </p>
    </NativeSheetSurface>
  ),
};
