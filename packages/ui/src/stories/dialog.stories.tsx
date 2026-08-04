import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";

import {
  Button,
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "../index";

const meta = {
  component: Dialog,
  parameters: {
    docs: {
      description: {
        component:
          "The shared Radix/shadcn dialog primitive. Product workflows compose this interface outside Storybook.",
      },
    },
    layout: "centered",
  },
  title: "Primitives/Dialog",
} satisfies Meta<typeof Dialog>;

export default meta;
type Story = StoryObj<typeof meta>;

function DialogExample({
  container,
  position = "viewport",
}: {
  container?: HTMLElement | null;
  position?: "container" | "viewport";
}) {
  return (
    <Dialog defaultOpen>
      <DialogTrigger asChild>
        <Button>Open dialog</Button>
      </DialogTrigger>
      <DialogContent container={container} position={position}>
        <DialogTitle className="m-0 text-[14px] font-bold">
          Dialog title
        </DialogTitle>
        <DialogDescription className="mt-1 mb-0 text-[10px] leading-4 text-pearl-muted">
          Use this space to explain the action and its consequences.
        </DialogDescription>
        <div className="mt-4 flex justify-end gap-2">
          <DialogClose asChild>
            <Button variant="secondary">Cancel</Button>
          </DialogClose>
          <DialogClose asChild>
            <Button>Continue</Button>
          </DialogClose>
        </div>
      </DialogContent>
    </Dialog>
  );
}

export const Default: Story = {
  render: () => <DialogExample />,
};

function ContainedDialogExample() {
  const [container, setContainer] = useState<HTMLDivElement | null>(null);

  return (
    <div
      className="relative grid h-[420px] w-[520px] max-w-full place-items-center overflow-hidden rounded-panel border border-pearl-border bg-panel-glass"
      ref={setContainer}
    >
      <DialogExample container={container} position="container" />
    </div>
  );
}

export const Contained: Story = {
  render: () => <ContainedDialogExample />,
};
