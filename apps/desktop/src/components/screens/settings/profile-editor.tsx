import { Button, ProfileCard } from "@touchgrass/ui";
import { useState } from "react";

import { DisplayNameInput } from "@/components/display-name-input";
import { useCopyText } from "@/components/use-copy-text";

type ProfileEditorProps = {
  className?: string;
  displayName: string;
  onDisplayNameChange?:
    | ((displayName: string) => boolean | Promise<boolean> | void)
    | undefined;
  touchGrassId: string;
};

function ProfileEditor(props: ProfileEditorProps) {
  const [editing, setEditing] = useState(false);
  const [draftDisplayName, setDraftDisplayName] = useState(props.displayName);
  const [saveFailed, setSaveFailed] = useState(false);
  const [saving, setSaving] = useState(false);
  const { copyStatus, copyText } = useCopyText(props.touchGrassId);
  const initialLetter =
    props.displayName.trim().slice(0, 1).toUpperCase() || "?";

  function beginEditing() {
    setDraftDisplayName(props.displayName);
    setSaveFailed(false);
    setEditing(true);
  }

  function cancelEditing() {
    setDraftDisplayName(props.displayName);
    setSaveFailed(false);
    setEditing(false);
  }

  async function saveDisplayName() {
    const displayName = draftDisplayName.trim();
    if (
      !displayName ||
      displayName.length > 40 ||
      props.onDisplayNameChange === undefined
    ) {
      return;
    }
    if (displayName === props.displayName) {
      setEditing(false);
      return;
    }
    setSaveFailed(false);
    setSaving(true);
    setEditing(false);
    try {
      const saved = await props.onDisplayNameChange(displayName);
      if (saved === false) {
        setSaveFailed(true);
        setEditing(true);
        return;
      }
    } catch {
      setSaveFailed(true);
      setEditing(true);
    } finally {
      setSaving(false);
    }
  }

  if (editing) {
    return (
      <ProfileCard className={props.className} data-profile-state="editing">
        <form
          className="grid gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void saveDisplayName();
          }}
        >
          <label>
            <small className="mb-1.5 block text-[9px] font-semibold text-sheet-muted">
              Display name
            </small>
            <DisplayNameInput
              aria-label="Display name"
              autoFocus
              disabled={saving}
              maxLength={40}
              onChange={(event) => setDraftDisplayName(event.target.value)}
              value={draftDisplayName}
            />
          </label>
          <small
            aria-live="polite"
            className="text-[9px] text-sheet-muted"
            data-display-name-save-state={saveFailed ? "failed" : "idle"}
          >
            {saveFailed ? "Display Name could not be saved. Try again." : ""}
          </small>
          <div className="flex justify-end gap-1.5">
            <Button
              disabled={saving}
              onClick={cancelEditing}
              size="quiet"
              type="button"
              variant="ghost"
            >
              Cancel
            </Button>
            <Button
              disabled={
                saving ||
                draftDisplayName.trim().length === 0 ||
                draftDisplayName.trim().length > 40
              }
              size="quiet"
              type="submit"
            >
              {saving ? "Saving…" : "Save"}
            </Button>
          </div>
        </form>
      </ProfileCard>
    );
  }

  return (
    <ProfileCard
      avatarLabel={initialLetter}
      className={props.className}
      data-profile-state={saving ? "saving" : "saved"}
      displayName={
        <strong className="mt-0.5 block truncate text-[13px]">
          {props.displayName}
        </strong>
      }
      displayNameAction={
        props.onDisplayNameChange === undefined ? undefined : (
          <Button
            disabled={saving}
            onClick={beginEditing}
            size="quiet"
            type="button"
            variant="ghost"
          >
            {saving ? "Saving…" : "Edit"}
          </Button>
        )
      }
      touchGrassId={
        <span className="mt-0.5 inline-flex min-w-0 items-center gap-1.5">
          <Button
            aria-label={`Copy TouchGrass ID ${props.touchGrassId}`}
            className="-ml-1.5 max-w-full font-mono text-[10px] font-bold text-sheet-ink"
            data-copy-status={copyStatus}
            onClick={() => void copyText()}
            size="quiet"
            title={
              copyStatus === "copied"
                ? "Copied"
                : copyStatus === "unavailable"
                  ? "Copy unavailable"
                  : "Copy TouchGrass ID"
            }
            type="button"
            variant="ghost"
          >
            <span className="truncate">{props.touchGrassId}</span>
          </Button>
          <span
            aria-live="polite"
            className="font-mono text-[8px] text-sheet-ink"
            data-copy-feedback={copyStatus}
          >
            {copyStatus === "copied"
              ? "Copied"
              : copyStatus === "unavailable"
                ? "Unavailable"
                : ""}
          </span>
        </span>
      }
      touchGrassIdDescription="Your permanent public ID."
    />
  );
}

export { ProfileEditor };
export type { ProfileEditorProps };
