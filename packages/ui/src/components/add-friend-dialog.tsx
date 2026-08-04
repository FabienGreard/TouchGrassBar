import { useRef, useState } from "react";

import { normalizeTouchGrassId } from "#lib/touchgrass-id";
import { Button } from "./button";
import { Input } from "./input";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "../internal/dialog";

type AddFriendDialogProps = {
  defaultTouchGrassId?: string;
  onOpenChange: (open: boolean) => void;
  open: boolean;
  portalContainer?: HTMLElement | null | undefined;
};

function AddFriendDialog({
  defaultTouchGrassId = "",
  onOpenChange,
  open,
  portalContainer = null,
}: AddFriendDialogProps) {
  const [touchGrassId, setTouchGrassId] = useState(defaultTouchGrassId);
  const [submitted, setSubmitted] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
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
          inputRef.current?.focus();
        }}
        position={portalContainer ? "container" : "viewport"}
      >
        <div className="relative px-8 text-center">
          <DialogTitle className="m-0 text-[14px] font-bold">
            Add a Tokenmaxxer
          </DialogTitle>
          <DialogDescription className="mt-1 mb-0 text-[10px] leading-4 text-cream-muted contrast-more:text-cream-ink">
            Enter their TouchGrass ID to add them to your board.
          </DialogDescription>
          <span className="absolute -top-1 -right-1 text-[16px]">
            <DialogClose asChild>
              <Button
                aria-label="Close add friend dialog"
                size="icon"
                type="button"
                variant="quiet"
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
            className="block text-[9px] font-semibold text-cream-muted contrast-more:text-cream-ink"
            htmlFor="touchgrass-friend-id"
          >
            TouchGrass ID
          </label>
          <Input
            aria-describedby="touchgrass-friend-id-help"
            autoComplete="off"
            className="mt-1.5 font-mono uppercase"
            id="touchgrass-friend-id"
            onChange={(event) => {
              setTouchGrassId(event.target.value);
              setSubmitted(false);
            }}
            placeholder="TG-ABC123"
            ref={inputRef}
            spellCheck={false}
            type="text"
            value={touchGrassId}
          />
          <small
            className="mt-1.5 block min-h-4 text-[8px] leading-4 text-cream-muted contrast-more:text-cream-ink"
            id="touchgrass-friend-id-help"
          >
            {submitted
              ? "Friend lookup is not connected yet."
              : touchGrassId && !valid
                ? "Use the format TG-ABC123."
                : "You can find the ID beside their Doomerboard name."}
          </small>
          <div className="mt-3 flex justify-end gap-2">
            <DialogClose asChild>
              <Button size="default" type="button" variant="quiet">
                Cancel
              </Button>
            </DialogClose>
            <Button disabled={!valid} size="default" type="submit">
              Add friend
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export { AddFriendDialog };
export type { AddFriendDialogProps };
