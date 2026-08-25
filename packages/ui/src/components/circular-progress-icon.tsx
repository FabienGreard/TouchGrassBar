import type { ComponentProps } from "react";

import { cn } from "../lib/utils";

type CircularProgressIconProps = Omit<ComponentProps<"span">, "children"> & {
  progress?: number | null | undefined;
  showCheck?: boolean | undefined;
};

function CircularProgressIcon({
  className,
  progress,
  showCheck = false,
  ...props
}: CircularProgressIconProps) {
  const determinate = typeof progress === "number";
  const percent = determinate ? Math.min(100, Math.max(0, Math.round(progress))) : null;

  return (
    <span
      className={cn("relative inline-grid size-[22px] shrink-0 place-items-center", className)}
      data-center={showCheck ? "check" : percent === null ? "empty" : "percentage"}
      data-progress={percent ?? undefined}
      data-slot="circular-progress-icon"
      data-state={determinate ? "determinate" : "indeterminate"}
      {...props}
    >
      <span
        className={cn(
          "absolute inset-0 grid place-items-center",
          !determinate && "animate-spin motion-reduce:animate-none",
        )}
      >
        <svg aria-hidden="true" className="size-[22px] overflow-visible" viewBox="0 0 24 24">
          <circle
            cx="12"
            cy="12"
            fill="none"
            opacity="0.22"
            r="9"
            stroke="currentColor"
            strokeWidth="1.6"
          />
          <circle
            cx="12"
            cy="12"
            fill="none"
            pathLength="100"
            r="9"
            stroke="currentColor"
            strokeDasharray={percent === null ? "28 72" : `${percent} ${100 - percent}`}
            strokeLinecap="round"
            strokeWidth="1.6"
            transform="rotate(-90 12 12)"
          />
        </svg>
      </span>
      {percent === null ? (
        showCheck ? (
          <svg
            aria-hidden="true"
            className="relative size-[22px]"
            data-slot="circular-progress-check"
            viewBox="0 0 24 24"
          >
            <path
              d="m8.25 12.25 2.45 2.35 5.05-5.2"
              fill="none"
              stroke="currentColor"
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeWidth="1.8"
            />
          </svg>
        ) : null
      ) : (
        <svg aria-hidden="true" className="relative size-[22px]" viewBox="0 0 24 24">
          <text
            dominantBaseline="central"
            fill="currentColor"
            fontFamily="ui-monospace, SFMono-Regular, Menlo, monospace"
            fontSize={percent === 100 ? "5.75" : "6.25"}
            fontWeight="750"
            textAnchor="middle"
            x="12"
            y="12"
          >
            {percent}%
          </text>
        </svg>
      )}
    </span>
  );
}

export { CircularProgressIcon };
export type { CircularProgressIconProps };
