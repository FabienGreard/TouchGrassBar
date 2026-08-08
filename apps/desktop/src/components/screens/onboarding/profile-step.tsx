import { ProfileCard } from "@touchgrass/ui";

import { DisplayNameInput } from "@/components/display-name-input";

function ProfileStep({
  displayName,
  onDisplayNameChange,
}: {
  displayName: string;
  onDisplayNameChange: (displayName: string) => void;
}) {
  const initialLetter = displayName.trim().slice(0, 1).toUpperCase() || "?";

  return (
    <div className="grid gap-3">
      <ProfileCard
        avatarLabel={initialLetter}
        data-profile-state="draft"
        displayName={
          <DisplayNameInput
            aria-label="Display name"
            className="mt-1 h-8"
            maxLength={40}
            onChange={(event) => onDisplayNameChange(event.target.value)}
            value={displayName}
          />
        }
        touchGrassId={
          <strong className="mt-1 block font-mono text-[10px] text-sheet-muted">
            Assigned after creation
          </strong>
        }
        touchGrassIdDescription="Your permanent public ID."
      />
      <div className="grid gap-1 px-1 text-[8px] leading-3.5 text-sheet-muted">
        <small>
          Other people can see your Display Name, TouchGrass ID, and daily
          scores on the Doomerboard. They cannot see your prompts,
          conversations, credentials, logs, or files.
        </small>
      </div>
    </div>
  );
}

export { ProfileStep };
