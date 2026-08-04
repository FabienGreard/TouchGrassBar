import { Input, ProfileCard } from "@touchgrass/ui";

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
          <Input
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
          Your name, ID, and daily scores are public. A Profile is required to
          continue.
        </small>
        <small>
          First sync may publish available totals for the current UTC day and
          previous 29; missing stays missing. Prompts, conversations,
          credentials, raw logs, and file paths stay on this Mac.
        </small>
      </div>
    </div>
  );
}

export { ProfileStep };
