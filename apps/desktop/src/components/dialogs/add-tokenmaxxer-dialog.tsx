import { Button, Dialog, DialogClose, DialogContent, DialogTitle, Input } from "@touchgrass/ui";
import { useEffect, useRef, useState } from "react";

import {
  addTokenmaxxerHelpText,
  normalizeTouchGrassId,
  validTouchGrassId,
  type AddTokenmaxxerDialogStatus,
} from "./add-tokenmaxxer";

type AddTokenmaxxerDialogProps = {
  defaultTouchGrassId?: string;
  onAddTokenmaxxer?: ((touchGrassId: string) => void) | undefined;
  onInputChange?: (() => void) | undefined;
  onOpenChange: (open: boolean) => void;
  open: boolean;
  portalContainer?: HTMLElement | null | undefined;
  status?: AddTokenmaxxerDialogStatus | undefined;
};

function AddTokenmaxxerDialog({
  defaultTouchGrassId = "",
  onAddTokenmaxxer = () => undefined,
  onInputChange = () => undefined,
  onOpenChange,
  open,
  portalContainer = null,
  status = "idle",
}: AddTokenmaxxerDialogProps) {
  const [touchGrassId, setTouchGrassId] = useState(defaultTouchGrassId);
  const touchGrassIdInputRef = useRef<HTMLInputElement>(null);
  const normalizedId = normalizeTouchGrassId(touchGrassId);
  const valid = validTouchGrassId(normalizedId);

  useEffect(() => {
    if (!open) setTouchGrassId(defaultTouchGrassId);
  }, [defaultTouchGrassId, open]);

  function handleOpenChange(nextOpen: boolean) {
    if (!nextOpen) {
      onInputChange();
    }
    onOpenChange(nextOpen);
  }

  return (
    <Dialog onOpenChange={handleOpenChange} open={open}>
      <DialogContent
        aria-describedby={undefined}
        container={portalContainer}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          touchGrassIdInputRef.current?.focus();
        }}
        position={portalContainer ? "container" : "viewport"}
      >
        <div className="relative px-8 text-center">
          <DialogTitle className="m-0 text-[14px] font-bold">Add a Tokenmaxxer</DialogTitle>
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
            if (valid && status !== "submitting") onAddTokenmaxxer(normalizedId);
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
              onInputChange();
            }}
            placeholder="TG-ABC234"
            ref={touchGrassIdInputRef}
            spellCheck={false}
            type="text"
            value={touchGrassId}
          />
          <small
            className="mt-1.5 block min-h-4 text-[8px] leading-4 text-pearl-muted contrast-more:text-pearl-ink"
            id="touchgrass-tokenmaxxer-id-help"
          >
            {touchGrassId && !valid
              ? addTokenmaxxerHelpText("invalid")
              : addTokenmaxxerHelpText(status)}
          </small>
          <div className="mt-3 flex justify-end gap-2">
            <DialogClose asChild>
              <Button size="default" type="button" variant="ghost">
                Cancel
              </Button>
            </DialogClose>
            <Button disabled={!valid || status === "submitting"} size="default" type="submit">
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
