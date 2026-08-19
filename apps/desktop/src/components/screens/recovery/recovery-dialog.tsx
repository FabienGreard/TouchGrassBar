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

import {
  focusAndSelectRecoveryInput,
  RECOVERY_KEY_PLACEHOLDER,
} from "@/components/screens/settings/recovery-key-input";

type ProfileRecoveryCredentials = {
  recoveryKey: string;
  touchGrassId: string;
};

type RecoveryDialogProps = {
  onOpenChange: (open: boolean) => void;
  onRecover: (
    credentials: ProfileRecoveryCredentials,
  ) => boolean | Promise<boolean>;
  open: boolean;
  portalContainer?: HTMLElement | null | undefined;
};

function RecoveryDialog({
  onOpenChange,
  onRecover,
  open,
  portalContainer = null,
}: RecoveryDialogProps) {
  const [touchGrassId, setTouchGrassId] = useState("");
  const [recoveryKey, setRecoveryKey] = useState("");
  const [recoveryFailed, setRecoveryFailed] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const touchGrassIdInputId = useId();
  const recoveryKeyInputId = useId();
  const recoveryKeyHelpId = `${recoveryKeyInputId}-help`;
  const touchGrassIdInput = useRef<HTMLInputElement>(null);

  function reset() {
    setTouchGrassId("");
    setRecoveryKey("");
    setRecoveryFailed(false);
    setSubmitting(false);
  }

  function handleOpenChange(nextOpen: boolean) {
    if (!nextOpen) reset();
    onOpenChange(nextOpen);
  }

  return (
    <Dialog onOpenChange={handleOpenChange} open={open}>
      <DialogContent
        container={portalContainer}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          focusAndSelectRecoveryInput(touchGrassIdInput.current);
        }}
        position={portalContainer ? "container" : "viewport"}
      >
        <div className="relative px-8 text-center">
          <DialogTitle className="m-0 text-[14px] font-bold">
            Recover from another Mac
          </DialogTitle>
          <DialogDescription className="mt-1 mb-0 text-[10px] leading-4 text-pearl-muted contrast-more:text-pearl-ink">
            Enter the Profile details stored on your other Mac.
          </DialogDescription>
          <span className="absolute -top-1 -right-1 text-[16px]">
            <DialogClose asChild>
              <Button
                aria-label="Close recovery dialog"
                disabled={submitting}
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
            if (!touchGrassId.trim() || !recoveryKey.trim() || submitting) {
              return;
            }
            setRecoveryFailed(false);
            setSubmitting(true);
            void Promise.resolve(
              onRecover({
                recoveryKey: recoveryKey.trim(),
                touchGrassId: touchGrassId.trim(),
              }),
            )
              .then((recovered) => {
                if (recovered) handleOpenChange(false);
                else setRecoveryFailed(true);
              })
              .catch(() => setRecoveryFailed(true))
              .finally(() => setSubmitting(false));
          }}
        >
          <label className="block" htmlFor={touchGrassIdInputId}>
            <small className="block text-[9px] font-semibold text-pearl-muted contrast-more:text-pearl-ink">
              TouchGrass ID
            </small>
            <Input
              autoComplete="off"
              autoFocus
              className="mt-1.5 font-mono tracking-[0.06em] uppercase"
              disabled={submitting}
              id={touchGrassIdInputId}
              onChange={(event) => {
                setTouchGrassId(event.target.value);
                setRecoveryFailed(false);
              }}
              onFocus={(event) => event.currentTarget.select()}
              placeholder="TG-…"
              ref={touchGrassIdInput}
              spellCheck={false}
              value={touchGrassId}
            />
          </label>

          <label className="mt-3 block" htmlFor={recoveryKeyInputId}>
            <small className="block text-[9px] font-semibold text-pearl-muted contrast-more:text-pearl-ink">
              Recovery Key
            </small>
            <Input
              aria-describedby={recoveryKeyHelpId}
              autoComplete="off"
              className="mt-1.5 font-mono tracking-[0.06em]"
              disabled={submitting}
              id={recoveryKeyInputId}
              onChange={(event) => {
                setRecoveryKey(event.target.value);
                setRecoveryFailed(false);
              }}
              onFocus={(event) => event.currentTarget.select()}
              placeholder={RECOVERY_KEY_PLACEHOLDER}
              spellCheck={false}
              type="password"
              value={recoveryKey}
            />
            <small
              className="mt-1.5 block text-[8px] leading-4 text-pearl-muted contrast-more:text-pearl-ink"
              id={recoveryKeyHelpId}
            >
              Find both in TouchGrassBar → Settings → Profile on your other Mac.
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

          {recoveryFailed ? (
            <div
              className="mt-4 rounded-[10px] border border-sheet-line bg-white/52 px-3 py-2.5"
              role="status"
            >
              <strong className="block text-[10px]">
                Profile recovery unavailable
              </strong>
              <small className="mt-1 block text-[8px] leading-4 text-pearl-muted">
                Check the details and try again.
              </small>
            </div>
          ) : null}

          <div className="mt-4 flex justify-end gap-2">
            <DialogClose asChild>
              <Button disabled={submitting} type="button" variant="ghost">
                Cancel
              </Button>
            </DialogClose>
            <Button
              disabled={
                !touchGrassId.trim() || !recoveryKey.trim() || submitting
              }
              type="submit"
            >
              {submitting ? "Recovering…" : "Recover on this Mac"}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export { RecoveryDialog };
export type { ProfileRecoveryCredentials, RecoveryDialogProps };
