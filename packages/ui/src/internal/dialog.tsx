import { Dialog as DialogPrimitive } from "radix-ui";
import type { ComponentProps } from "react";

import { cn } from "../lib/utils";

function Dialog(props: ComponentProps<typeof DialogPrimitive.Root>) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />;
}

function DialogClose(props: ComponentProps<typeof DialogPrimitive.Close>) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />;
}

function DialogDescription(
  props: ComponentProps<typeof DialogPrimitive.Description>,
) {
  return (
    <DialogPrimitive.Description data-slot="dialog-description" {...props} />
  );
}

function DialogTitle(props: ComponentProps<typeof DialogPrimitive.Title>) {
  return <DialogPrimitive.Title data-slot="dialog-title" {...props} />;
}

type DialogContentProps = ComponentProps<typeof DialogPrimitive.Content> & {
  container?: HTMLElement | null | undefined;
  position?: "container" | "viewport";
};

function DialogContent({
  className,
  container,
  position = "viewport",
  ...props
}: DialogContentProps) {
  const contained = position === "container";

  return (
    <DialogPrimitive.Portal container={container ?? undefined}>
      <DialogPrimitive.Overlay
        className={cn(
          "inset-0 z-50 bg-cream-ink/25 backdrop-blur-[3px] data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:animate-in data-[state=open]:fade-in-0 motion-reduce:animate-none",
          contained ? "absolute" : "fixed",
        )}
        data-slot="dialog-overlay"
      />
      <DialogPrimitive.Content
        className={cn(
          "top-1/2 left-1/2 z-50 w-[calc(100%_-_32px)] max-w-[340px] -translate-x-1/2 -translate-y-1/2 rounded-[14px] border border-cream-border bg-native-sheet p-4 text-cream-ink shadow-native-sheet outline-none data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95 motion-reduce:animate-none contrast-more:border-cream-ink contrast-more:bg-cream-highlight",
          contained ? "absolute" : "fixed",
          className,
        )}
        data-slot="dialog-content"
        {...props}
      />
    </DialogPrimitive.Portal>
  );
}

export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
};
