import type { Meta, StoryObj } from "@storybook/react-vite";

import { Input } from "../index";

const meta = {
  args: {
    disabled: false,
    placeholder: "TG-ABC123",
    type: "text",
  },
  argTypes: {
    className: { table: { disable: true } },
    disabled: { control: "boolean" },
    placeholder: { control: "text" },
    type: { control: "text" },
  },
  component: Input,
  decorators: [
    (Story) => (
      <div className="w-[300px]">
        <Story />
      </div>
    ),
  ],
  parameters: {
    docs: {
      description: {
        component:
          "The shared shadcn-style text input with the approved opaque white prototype surface and focus treatment.",
      },
    },
    layout: "centered",
  },
  title: "Primitives/Input",
} satisfies Meta<typeof Input>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const Focused: Story = {
  args: { autoFocus: true },
};

export const Invalid: Story = {
  args: {
    "aria-invalid": true,
    defaultValue: "not-an-id",
  },
};

export const Disabled: Story = {
  args: {
    defaultValue: "TG-ABC123",
    disabled: true,
  },
};
