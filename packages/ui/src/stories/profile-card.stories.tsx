import type { Meta, StoryObj } from "@storybook/react-vite";

import { Button, Input, ProfileCard } from "../index";

type ProfileCardStoryArgs = {
  variant: "draft" | "saved";
};

function ProfileCardStory({ variant }: ProfileCardStoryArgs) {
  if (variant === "draft") {
    return (
      <ProfileCard
        avatarLabel="F"
        displayName={
          <Input
            aria-label="Display name"
            className="mt-1 h-8"
            readOnly
            value="Fabien"
          />
        }
        touchGrassId={
          <strong className="mt-1 block font-mono text-[10px] text-sheet-muted">
            Assigned after creation
          </strong>
        }
        touchGrassIdDescription="Your permanent public ID."
      />
    );
  }

  return (
    <ProfileCard
      avatarLabel="F"
      displayName={
        <strong className="mt-0.5 block truncate text-[13px]">Fabien</strong>
      }
      displayNameAction={
        <Button size="quiet" type="button" variant="ghost">
          Edit
        </Button>
      }
      touchGrassId={
        <strong className="mt-1 block font-mono text-[10px]">
          #TG-7K4P9D
        </strong>
      }
      touchGrassIdAction={
        <Button size="quiet" type="button" variant="ghost">
          Copy ID
        </Button>
      }
      touchGrassIdDescription="Your permanent public ID."
    />
  );
}

const meta = {
  component: ProfileCardStory,
  decorators: [
    (Story) => (
      <div className="w-[520px] max-w-full">
        <Story />
      </div>
    ),
  ],
  title: "Components/Profile card",
} satisfies Meta<ProfileCardStoryArgs>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Saved: Story = { args: { variant: "saved" } };

export const Draft: Story = { args: { variant: "draft" } };
