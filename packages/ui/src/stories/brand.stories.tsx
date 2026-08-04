import type { Meta, StoryObj } from "@storybook/react-vite";

import {
  Brand,
  BrandMark,
  BrandWordmark,
  DesktopAppIcon,
} from "../index";

const meta = {
  component: Brand,
  parameters: {
    docs: {
      description: {
        component:
          "The complete TouchGrassBar brand: desktop app icon, compact lily mark, and joined wordmark. Lime remains the identifying signal through the wordmark underline, selected controls, actions, and focus states.",
      },
    },
  },
  title: "Foundation/Brand",
} satisfies Meta<typeof Brand>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Overview: Story = {};

export const Wordmark: Story = {
  render: () => <BrandWordmark />,
};

export const Marks: Story = {
  render: () => (
    <div className="flex items-end gap-6">
      <figure className="m-0 grid justify-items-center gap-2">
        <BrandMark size="panel" />
        <figcaption className="text-[11px] text-pearl-muted">Panel</figcaption>
      </figure>
      <figure className="m-0 grid justify-items-center gap-2">
        <BrandMark size="sidebar" />
        <figcaption className="text-[11px] text-pearl-muted">
          Sidebar
        </figcaption>
      </figure>
      <figure className="m-0 grid justify-items-center gap-2">
        <span className="grid size-12 place-items-center rounded-[10px] bg-pearl-ink">
          <BrandMark size="sidebar" tone="reversed" />
        </span>
        <figcaption className="text-[11px] text-pearl-muted">
          Reversed
        </figcaption>
      </figure>
    </div>
  ),
};

export const AppIcon: Story = {
  parameters: {
    docs: {
      description: {
        story:
          "The canonical frosted-pearl source used to generate the PNG and ICNS files in the Tauri desktop bundle.",
      },
    },
  },
  render: () => (
    <figure className="m-0 grid justify-items-center gap-3">
      <DesktopAppIcon
        alt="TouchGrassBar desktop application icon"
        size="large"
      />
      <figcaption className="text-[11px] text-pearl-muted">
        Desktop application icon
      </figcaption>
    </figure>
  ),
};
