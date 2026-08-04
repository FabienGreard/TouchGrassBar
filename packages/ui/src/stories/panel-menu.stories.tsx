import type { Meta, StoryObj } from "@storybook/react-vite";

import {
  Button,
  InviteIcon,
  PanelMenu,
  PanelMenuContent,
  PanelMenuItem,
  PanelMenuRadioGroup,
  PanelMenuRadioItem,
  PanelMenuTrigger,
  RefreshIcon,
  SettingsIcon,
} from "../index";

const meta = {
  args: {
    align: "center",
    side: "bottom",
    sideOffset: 8,
    size: "menu",
  },
  argTypes: {
    align: {
      control: "inline-radio",
      options: ["start", "center", "end"],
    },
    children: { table: { disable: true } },
    className: { table: { disable: true } },
    side: {
      control: "inline-radio",
      options: ["top", "right", "bottom", "left"],
    },
    sideOffset: {
      control: { max: 24, min: 0, step: 1, type: "range" },
    },
    size: {
      control: "inline-radio",
      options: ["menu", "query"],
    },
  },
  component: PanelMenuContent,
  parameters: {
    docs: {
      description: {
        component:
          "The shared warm-glass menu surface used for both panel actions and compact query selectors.",
      },
    },
    layout: "centered",
  },
  title: "Primitives/Panel menu",
} satisfies Meta<typeof PanelMenuContent>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Actions: Story = {
  render: (args) => (
    <PanelMenu defaultOpen>
      <PanelMenuTrigger asChild>
        <Button size="quiet" variant="quiet">
          Open actions
        </Button>
      </PanelMenuTrigger>
      <PanelMenuContent {...args}>
        <PanelMenuItem>
          <RefreshIcon aria-hidden="true" /> Force sync
        </PanelMenuItem>
        <PanelMenuItem>
          <InviteIcon aria-hidden="true" /> Invite a friend
        </PanelMenuItem>
        <PanelMenuItem>
          <SettingsIcon aria-hidden="true" /> Settings…
        </PanelMenuItem>
      </PanelMenuContent>
    </PanelMenu>
  ),
};

export const Query: Story = {
  args: { size: "query" },
  render: (args) => (
    <PanelMenu defaultOpen>
      <PanelMenuTrigger asChild>
        <Button size="quiet" variant="quiet">
          Today
        </Button>
      </PanelMenuTrigger>
      <PanelMenuContent {...args}>
        <PanelMenuRadioGroup value="today">
          <PanelMenuRadioItem value="today">Today</PanelMenuRadioItem>
          <PanelMenuRadioItem value="week">7 days</PanelMenuRadioItem>
          <PanelMenuRadioItem value="month">30 days</PanelMenuRadioItem>
        </PanelMenuRadioGroup>
      </PanelMenuContent>
    </PanelMenu>
  ),
};
