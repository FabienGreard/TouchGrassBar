import type { Meta, StoryObj } from "@storybook/react-vite";

import { Brand, PanelShell } from "../index";

const meta = {
  args: { glass: false },
  argTypes: {
    children: { table: { disable: true } },
    className: { table: { disable: true } },
    glass: { control: "boolean" },
  },
  component: PanelShell,
  parameters: {
    docs: {
      description: {
        component:
          "The shared 402px menubar panel surface, including its border, warm glass treatment, contrast fallback, and optional native backdrop blur.",
      },
    },
  },
  title: "Surfaces/Panel shell",
} satisfies Meta<typeof PanelShell>;

export default meta;
type Story = StoryObj<typeof meta>;

export const PanelSurface: Story = {
  render: (args) => (
    <PanelShell {...args}>
      <header className="border-b border-pearl-line bg-panel-header px-4 py-4">
        <Brand />
      </header>
      <section className="bg-provider-row px-4 py-6 text-[11px] text-pearl-muted">
        Shared panel surface
      </section>
    </PanelShell>
  ),
};
