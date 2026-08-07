import { Button, Input, ProfileCard } from "@touchgrass/ui";
import { useState } from "react";

import { useCopyText } from "@/components/use-copy-text";

type ProfileEditorProps = {
  className?: string;
  displayName: string;
  onDisplayNameChange?: ((displayName: string) => void) | undefined;
  touchGrassId: string;
};

function ProfileEditor(props: ProfileEditorProps) {
  const [editing, setEditing] = useState(false);
  const [draftDisplayName, setDraftDisplayName] = useState(props.displayName);
  const { copyStatus, copyText } = useCopyText(props.touchGrassId);
  const initialLetter =
    props.displayName.trim().slice(0, 1).toUpperCase() || "?";

  function beginEditing() {
    setDraftDisplayName(props.displayName);
    setEditing(true);
  }

  function cancelEditing() {
    setDraftDisplayName(props.displayName);
    setEditing(false);
  }

  function saveDisplayName() {
    const displayName = draftDisplayName.trim();
    if (!displayName || props.onDisplayNameChange === undefined) return;
    props.onDisplayNameChange(displayName);
    setEditing(false);
  }

  if (editing) {
    return (
      <ProfileCard className={props.className} data-profile-state="editing">
        <form
          className="grid gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            saveDisplayName();
          }}
        >
          <label>
            <small className="mb-1.5 block text-[9px] font-semibold text-sheet-muted">
              Display name
            </small>
            <Input
              aria-label="Display name"
              autoFocus
              onChange={(event) => setDraftDisplayName(event.target.value)}
              value={draftDisplayName}
            />
          </label>
          <div className="flex justify-end gap-1.5">
            <Button
              onClick={cancelEditing}
              size="quiet"
              type="button"
              variant="ghost"
            >
              Cancel
            </Button>
            <Button
              disabled={draftDisplayName.trim().length === 0}
              size="quiet"
              type="submit"
            >
              Save
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
      data-profile-state="saved"
      displayName={
        <strong className="mt-0.5 block truncate text-[13px]">
          {props.displayName}
        </strong>
      }
      displayNameAction={
        props.onDisplayNameChange === undefined ? undefined : (
          <Button
            onClick={beginEditing}
            size="quiet"
            type="button"
            variant="ghost"
          >
            Edit
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
