import type { Meta, StoryObj } from "@storybook/react-vite";

import {
  Brand,
  NativeWindow,
  NativeWindowContent,
  NativeWindowNav,
  NativeWindowNavItem,
  NativeWindowSidebar,
} from "../index";

const meta = {
  component: NativeWindow,
  parameters: {
    docs: {
      description: {
        component:
          "The shared standalone macOS window frame, sidebar, navigation, and content primitives used by onboarding and settings.",
      },
    },
    layout: "fullscreen",
  },
  title: "Surfaces/Native window",
} satisfies Meta<typeof NativeWindow>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Settings: Story = {
  render: () => (
    <NativeWindow>
      <NativeWindowSidebar>
        <Brand />
        <NativeWindowNav>
          <NativeWindowNavItem aria-current="page">General</NativeWindowNavItem>
          <NativeWindowNavItem aria-disabled="true">
            Providers
          </NativeWindowNavItem>
        </NativeWindowNav>
      </NativeWindowSidebar>
      <NativeWindowContent>
        <h1 className="m-0 text-[22px] tracking-[-0.04em]">Settings</h1>
        <p className="mt-2 text-[11px] text-sheet-muted">
          Standalone native sheet
        </p>
      </NativeWindowContent>
    </NativeWindow>
  ),
};
