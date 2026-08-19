import type { Meta, StoryObj } from "@storybook/react-vite";
import { useEffect, useState, type ComponentProps } from "react";

import { SegmentedControl, SegmentedControlItem } from "../index";

function AudienceControl({
  disabled,
  selection,
}: {
  disabled: boolean;
  selection: "global" | "mine";
}) {
  const [currentSelection, setCurrentSelection] = useState(selection);

  useEffect(() => setCurrentSelection(selection), [selection]);

  return (
    <SegmentedControl
      aria-label="Doomerboard audience"
      onValueChange={(value) => setCurrentSelection(value as "global" | "mine")}
      value={currentSelection}
    >
      <SegmentedControlItem disabled={disabled} value="mine">
        Friends
      </SegmentedControlItem>
      <SegmentedControlItem disabled={disabled} value="global">
        Global
      </SegmentedControlItem>
    </SegmentedControl>
  );
}

type SegmentedControlStoryArgs = Omit<
  ComponentProps<typeof SegmentedControl>,
  "children" | "onValueChange" | "value"
> & {
  disabled: boolean;
  selection: "global" | "mine";
};

const meta = {
  args: { disabled: false, selection: "global" },
  argTypes: {
    className: { table: { disable: true } },
    disabled: { control: "boolean" },
    selection: {
      control: "inline-radio",
      options: ["mine", "global"],
    },
  },
  component: SegmentedControl,
  parameters: {
    docs: {
      description: {
        component: "The shared compact segmented control used to switch Doomerboard audiences.",
      },
    },
  },
  render: ({ disabled, selection }) => (
    <AudienceControl disabled={disabled} selection={selection} />
  ),
  title: "Primitives/Segmented control",
} satisfies Meta<SegmentedControlStoryArgs>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Audience: Story = {};
