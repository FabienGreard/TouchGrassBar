type OnboardingStep = "finish" | "profile" | "providers";

const onboardingSteps = [
  {
    actionLabel: "Continue",
    description: "Use the coding providers already available on this Mac.",
    key: "providers",
    label: "Providers",
    title: "Connect your providers",
  },
  {
    actionLabel: "Continue",
    description: "Create your public Profile on this Mac.",
    key: "profile",
    label: "Profile",
    title: "Set up your Profile",
  },
  {
    actionLabel: "Finish setup",
    description: "You’re ready to use TouchGrassBar.",
    key: "finish",
    label: "Finish",
    title: "Finish setup",
  },
] as const;

export { onboardingSteps };
export type { OnboardingStep };
