import { Input, ProfileCard } from "@touchgrass/ui";
import { useState } from "react";

function ProfileStep({
  initialDisplayName = "",
}: {
  initialDisplayName?: string;
}) {
  const [displayName, setDisplayName] = useState(initialDisplayName);
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
            onChange={(event) => setDisplayName(event.target.value)}
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
      <small className="px-1 text-[8px] leading-3.5 text-sheet-muted">
        Your name, ID, and daily scores are public.
      </small>
    </div>
  );
}

export { ProfileStep };
