import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";

import {
  CheckIcon,
  NativeWindowNav,
  NativeWindowNavItem,
  NativeWindowNavStepMarker,
} from "../index";

const meta = {
  component: NativeWindowNav,
  parameters: {
    docs: {
      description: {
        component:
          "The shared sidebar navigation primitive used by settings and onboarding. It owns hover, selected, pressed, disabled, and keyboard-focus presentation.",
      },
    },
  },
  title: "Primitives/Sidebar navigation",
} satisfies Meta<typeof NativeWindowNav>;

export default meta;
type Story = StoryObj<typeof meta>;

function InteractiveNavigationPreview() {
  const [current, setCurrent] = useState("general");

  return (
    <div className="w-[220px] rounded-[12px] bg-sheet-sidebar p-4">
      <NativeWindowNav aria-label="Example sections" className="mt-0">
        {["general", "providers", "profile"].map((item) => (
          <NativeWindowNavItem asChild key={item}>
            <button
              aria-current={current === item ? "page" : undefined}
              className="w-full cursor-pointer border-0 bg-transparent text-left capitalize"
              onClick={() => setCurrent(item)}
              type="button"
            >
              {item}
            </button>
          </NativeWindowNavItem>
        ))}
      </NativeWindowNav>
    </div>
  );
}

export const Interactive: Story = {
  render: () => <InteractiveNavigationPreview />,
};

function StepNavigationPreview() {
  const steps = ["providers", "profile", "recovery"] as const;
  const [current, setCurrent] = useState<(typeof steps)[number]>("providers");
  const currentIndex = steps.indexOf(current);

  return (
    <div className="w-[220px] rounded-[12px] bg-sheet-sidebar p-4">
      <NativeWindowNav aria-label="Example onboarding steps" className="mt-0">
        {steps.map((item, index) => {
          const complete = index < currentIndex;
          return (
            <NativeWindowNavItem asChild key={item} variant="step">
              <button
                aria-current={current === item ? "step" : undefined}
                className="w-full cursor-pointer border-0 bg-transparent capitalize"
                onClick={() => setCurrent(item)}
                type="button"
              >
                <NativeWindowNavStepMarker complete={complete}>
                  {complete ? <CheckIcon size={11} /> : index + 1}
                </NativeWindowNavStepMarker>
                {item}
              </button>
            </NativeWindowNavItem>
          );
        })}
      </NativeWindowNav>
    </div>
  );
}

export const Steps: Story = {
  render: () => <StepNavigationPreview />,
};
