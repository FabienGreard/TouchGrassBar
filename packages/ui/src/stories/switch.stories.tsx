import type { Meta, StoryObj } from "@storybook/react-vite";

import { Switch } from "../index";

const meta = {
  args: {
    "aria-label": "Launch at login",
    defaultChecked: true,
    disabled: false,
    size: "default",
  },
  argTypes: {
    "aria-label": { control: "text" },
    defaultChecked: { control: "boolean" },
    disabled: { control: "boolean" },
    size: {
      control: "inline-radio",
      options: ["sm", "default"],
    },
  },
  component: Switch,
  parameters: {
    docs: {
      description: {
        component:
          "The compact settings switch, using the same action-green selection treatment as the panel controls.",
      },
    },
  },
  render: ({ defaultChecked, ...args }) => (
    <Switch
      defaultChecked={defaultChecked ?? false}
      key={`${String(defaultChecked)}-${String(args.disabled)}-${args.size}`}
      {...args}
    />
  ),
  title: "Primitives/Switch",
} satisfies Meta<typeof Switch>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Setting: Story = {};

export const UnavailableSetting: Story = {
  args: {
    "aria-label": "Unavailable setting",
    defaultChecked: false,
    disabled: true,
  },
};
