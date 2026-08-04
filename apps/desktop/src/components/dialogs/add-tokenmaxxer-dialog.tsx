import {
  Button,
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
  Input,
} from "@touchgrass/ui";
import { useRef, useState } from "react";

import { normalizeTouchGrassId } from "./add-tokenmaxxer";

type AddTokenmaxxerDialogProps = {
  defaultTouchGrassId?: string;
  onOpenChange: (open: boolean) => void;
  open: boolean;
  portalContainer?: HTMLElement | null | undefined;
};

function AddTokenmaxxerDialog({
  defaultTouchGrassId = "",
  onOpenChange,
  open,
  portalContainer = null,
}: AddTokenmaxxerDialogProps) {
  const [touchGrassId, setTouchGrassId] = useState(defaultTouchGrassId);
  const [submitted, setSubmitted] = useState(false);
  const touchGrassIdInputRef = useRef<HTMLInputElement>(null);
  const normalizedId = normalizeTouchGrassId(touchGrassId);
  const valid = /^TG-[A-Z0-9]{6}$/.test(normalizedId);

  function handleOpenChange(nextOpen: boolean) {
    if (!nextOpen) {
      setTouchGrassId(defaultTouchGrassId);
      setSubmitted(false);
    }
    onOpenChange(nextOpen);
  }

  return (
    <Dialog onOpenChange={handleOpenChange} open={open}>
      <DialogContent
        container={portalContainer}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          touchGrassIdInputRef.current?.focus();
        }}
        position={portalContainer ? "container" : "viewport"}
      >
        <div className="relative px-8 text-center">
          <DialogTitle className="m-0 text-[14px] font-bold">
            Add a Tokenmaxxer
          </DialogTitle>
          <DialogDescription className="mt-1 mb-0 text-[10px] leading-4 text-pearl-muted contrast-more:text-pearl-ink">
            Enter their TouchGrass ID to add them to your board.
          </DialogDescription>
          <span className="absolute -top-1 -right-1 text-[16px]">
            <DialogClose asChild>
              <Button
                aria-label="Close add Tokenmaxxer dialog"
                size="icon"
                type="button"
                variant="ghost"
              >
                ×
              </Button>
            </DialogClose>
          </span>
        </div>

        <form
          className="mt-4"
          onSubmit={(event) => {
            event.preventDefault();
            if (valid) setSubmitted(true);
          }}
        >
          <label
            className="block text-[9px] font-semibold text-pearl-muted contrast-more:text-pearl-ink"
            htmlFor="touchgrass-tokenmaxxer-id"
          >
            TouchGrass ID
          </label>
          <Input
            aria-describedby="touchgrass-tokenmaxxer-id-help"
            autoComplete="off"
            className="mt-1.5 font-mono uppercase"
            id="touchgrass-tokenmaxxer-id"
            onChange={(event) => {
              setTouchGrassId(event.target.value);
              setSubmitted(false);
            }}
            placeholder="TG-ABC123"
            ref={touchGrassIdInputRef}
            spellCheck={false}
            type="text"
            value={touchGrassId}
          />
          <small
            className="mt-1.5 block min-h-4 text-[8px] leading-4 text-pearl-muted contrast-more:text-pearl-ink"
            id="touchgrass-tokenmaxxer-id-help"
          >
            {submitted
              ? "Tokenmaxxer lookup is not connected yet."
              : touchGrassId && !valid
                ? "Use the format TG-ABC123."
                : "You can find the ID beside their Doomerboard name."}
          </small>
          <div className="mt-3 flex justify-end gap-2">
            <DialogClose asChild>
              <Button size="default" type="button" variant="ghost">
                Cancel
              </Button>
            </DialogClose>
            <Button disabled={!valid} size="default" type="submit">
              Add Tokenmaxxer
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export { AddTokenmaxxerDialog };
export type { AddTokenmaxxerDialogProps };
