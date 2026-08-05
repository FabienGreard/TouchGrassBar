import {
  Button,
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
  Input,
} from "@touchgrass/ui";
import { useId, useRef, useState } from "react";

type RecoverySheetPreviewProps = {
  onOpenChange: (open: boolean) => void;
  open: boolean;
};

function RecoverySheetPreview({
  onOpenChange,
  open,
}: RecoverySheetPreviewProps) {
  const [recoveryKey, setRecoveryKey] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const inputId = useId();
  const helpId = `${inputId}-help`;
  const inputRef = useRef<HTMLInputElement>(null);

  function handleOpenChange(nextOpen: boolean) {
    if (!nextOpen) {
      setRecoveryKey("");
      setSubmitted(false);
    }
    onOpenChange(nextOpen);
  }

  return (
    <Dialog onOpenChange={handleOpenChange} open={open}>
      <DialogContent
        data-dev-only="recovery-sheet-preview"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          inputRef.current?.focus();
        }}
      >
        <div className="relative px-8 text-center">
          <DialogTitle className="m-0 text-[14px] font-bold">
            Recover from another Mac
          </DialogTitle>
          <DialogDescription className="mt-1 mb-0 text-[10px] leading-4 text-pearl-muted contrast-more:text-pearl-ink">
            Enter the Recovery Key stored on your other Mac.
          </DialogDescription>
          <span className="absolute -top-1 -right-1 text-[16px]">
            <DialogClose asChild>
              <Button
                aria-label="Close recovery dialog"
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
            if (!recoveryKey.trim()) return;
            setSubmitted(true);
          }}
        >
          <label className="block" htmlFor={inputId}>
            <small className="block text-[9px] font-semibold text-pearl-muted contrast-more:text-pearl-ink">
              Recovery Key
            </small>
            <Input
              aria-describedby={helpId}
              autoComplete="off"
              className="mt-1.5 font-mono tracking-[0.06em] uppercase"
              id={inputId}
              onChange={(event) => {
                setRecoveryKey(event.target.value);
                setSubmitted(false);
              }}
              placeholder="TG-RK-••••-••••-••••"
              ref={inputRef}
              spellCheck={false}
              type="password"
              value={recoveryKey}
            />
            <small
              className="mt-1.5 block text-[8px] leading-4 text-pearl-muted contrast-more:text-pearl-ink"
              id={helpId}
            >
              Find it in TouchGrassBar → Settings → Profile on your other Mac.
            </small>
          </label>

          <div className="mt-3 rounded-[10px] border border-[#d7bd83] bg-[#fff4d9] px-3 py-2.5 text-left">
            <strong className="block text-[10px] text-[#4f3912]">
              One-use Recovery Key
            </strong>
            <small className="mt-1 block text-[8px] leading-3.5 text-[#6d5a32]">
              This key works once. After recovery, this Mac takes over and the
              other Mac stops syncing. A new Recovery Key is created here.
            </small>
          </div>

          {submitted ? (
            <div
              className="mt-4 rounded-[10px] border border-sheet-line bg-white/52 px-3 py-2.5"
              role="status"
            >
              <strong className="block text-[10px]">Not connected yet</strong>
              <small className="mt-1 block text-[8px] leading-4 text-pearl-muted">
                Recovery is not connected to the native or backend flow in this
                build.
              </small>
            </div>
          ) : null}

          <div className="mt-4 flex justify-end gap-2">
            <DialogClose asChild>
              <Button type="button" variant="ghost">
                Cancel
              </Button>
            </DialogClose>
            <Button disabled={!recoveryKey.trim()} type="submit">
              Recover on this Mac
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export { RecoverySheetPreview };
