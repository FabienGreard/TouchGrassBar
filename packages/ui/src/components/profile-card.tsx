import type { ComponentProps, ReactNode } from "react";

import { cn } from "#lib/utils";

type ProfileCardProps = Omit<ComponentProps<"section">, "children"> &
  (
    | {
        children: ReactNode;
        avatarLabel?: never;
        displayName?: never;
        displayNameAction?: never;
        touchGrassId?: never;
        touchGrassIdAction?: never;
        touchGrassIdDescription?: never;
      }
    | {
        children?: never;
        avatarLabel: ReactNode;
        displayName: ReactNode;
        displayNameAction?: ReactNode;
        touchGrassId?: ReactNode;
        touchGrassIdAction?: ReactNode;
        touchGrassIdDescription?: ReactNode;
      }
  );

function ProfileCardFrame({ className, ...props }: ComponentProps<"section">) {
  return (
    <section
      className={cn(
        "rounded-[12px] border border-sheet-line bg-white/38 px-4 py-4 shadow-surface",
        className,
      )}
      data-slot="profile-card"
      {...props}
    />
  );
}

function ProfileCard(props: ProfileCardProps) {
  if ("children" in props) {
    const { children, ...frameProps } = props;
    return <ProfileCardFrame {...frameProps}>{children}</ProfileCardFrame>;
  }

  const {
    avatarLabel,
    displayName,
    displayNameAction,
    touchGrassId,
    touchGrassIdAction,
    touchGrassIdDescription,
    ...frameProps
  } = props;

  return (
    <ProfileCardFrame {...frameProps}>
      <div className="flex items-center gap-3">
        <span className="grid size-9 shrink-0 place-items-center rounded-full bg-action text-[12px] font-bold text-accent-foreground">
          {avatarLabel}
        </span>
        <div className="min-w-0 flex-1">
          <small className="block text-[8px] font-semibold tracking-[0.06em] text-sheet-muted uppercase">
            Display name
          </small>
          {displayName}
        </div>
        {displayNameAction}
      </div>
      {touchGrassId === undefined ? null : (
        <div className="mt-4 flex items-center gap-3 border-t border-sheet-line pt-3">
          <div className="min-w-0 flex-1">
            <small className="block text-[8px] font-semibold tracking-[0.06em] text-sheet-muted uppercase">
              TouchGrass ID
            </small>
            {touchGrassId}
            {touchGrassIdDescription === undefined ? null : (
              <small className="mt-1 block text-[9px] leading-4 text-sheet-muted">
                {touchGrassIdDescription}
              </small>
            )}
          </div>
          {touchGrassIdAction}
        </div>
      )}
    </ProfileCardFrame>
  );
}

export { ProfileCard };
export type { ProfileCardProps };
