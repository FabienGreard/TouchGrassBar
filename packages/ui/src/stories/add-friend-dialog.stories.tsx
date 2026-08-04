import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";

import {
  AddFriendDialog,
  Button,
  PanelShell,
  type AddFriendDialogProps,
} from "../index";

function DialogStory(props: Pick<AddFriendDialogProps, "defaultTouchGrassId">) {
  const [open, setOpen] = useState(true);

  return (
    <>
      <Button onClick={() => setOpen(true)}>Add by ID</Button>
      <AddFriendDialog {...props} onOpenChange={setOpen} open={open} />
    </>
  );
}

function PanelContainedDialogStory() {
  const [container, setContainer] = useState<HTMLElement | null>(null);
  const [open, setOpen] = useState(true);

  return (
    <PanelShell className="relative grid h-[620px] place-items-center" ref={setContainer}>
      <Button onClick={() => setOpen(true)}>Add by ID</Button>
      <AddFriendDialog
        onOpenChange={setOpen}
        open={open}
        portalContainer={container}
      />
    </PanelShell>
  );
}

const meta = {
  args: {
    defaultTouchGrassId: "",
    onOpenChange: () => undefined,
    open: true,
  },
  argTypes: {
    defaultTouchGrassId: { control: "text" },
    onOpenChange: { table: { disable: true } },
    open: { table: { disable: true } },
    portalContainer: { table: { disable: true } },
  },
  component: AddFriendDialog,
  parameters: {
    docs: {
      description: {
        component:
          "The shared shadcn/Radix dialog used by the unavailable Doomerboard, empty Tokenmaxxers view, and panel menu invite action.",
      },
    },
    layout: "centered",
  },
  render: ({ defaultTouchGrassId }) => (
    <DialogStory
      defaultTouchGrassId={defaultTouchGrassId ?? ""}
      key={defaultTouchGrassId}
    />
  ),
  title: "Surfaces/Add friend dialog",
} satisfies Meta<typeof AddFriendDialog>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Empty: Story = {};

export const InvalidId: Story = {
  args: { defaultTouchGrassId: "not-an-id" },
};

export const ValidId: Story = {
  args: { defaultTouchGrassId: "#tg-abc123" },
};

export const PanelContained: Story = {
  render: () => <PanelContainedDialogStory />,
};
