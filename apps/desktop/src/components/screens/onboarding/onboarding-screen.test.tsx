import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";

import { OnboardingScreen } from "./onboarding-screen";

const providers = [
  { displayName: "Codex", provider: "codex", state: "unavailable" },
  { displayName: "Claude", provider: "claude", state: "unavailable" },
] as const;

describe("onboarding screen", () => {
  test("uses the approved native-sheet composition", () => {
    const markup = renderToStaticMarkup(
      <OnboardingScreen providers={providers} />,
    );
    const profileMarkup = renderToStaticMarkup(
      <OnboardingScreen appVersion="1.3.2" initialStep="profile" />,
    );
    const finishMarkup = renderToStaticMarkup(
      <OnboardingScreen initialStep="finish" setupState="required" />,
    );
    const unavailableFinishMarkup = renderToStaticMarkup(
      <OnboardingScreen initialStep="finish" />,
    );
    const pendingFinishMarkup = renderToStaticMarkup(
      <OnboardingScreen
        initialStep="finish"
        onFinish={() => undefined}
        setupState="profile-pending"
      />,
    );
    const submittingFinishMarkup = renderToStaticMarkup(
      <OnboardingScreen
        initialStep="finish"
        onFinish={() => undefined}
        setupState="profile-pending"
        submissionState="submitting"
      />,
    );

    expect(markup).toContain('data-slot="native-window"');
    expect(markup).toContain("Connect your providers");
    expect(markup).toContain('aria-label="Onboarding steps"');
    expect(markup).toContain('data-slot="scroll-area-root"');
    expect(markup.match(/data-slot="provider-mark"/g)).toHaveLength(2);
    expect(markup).toContain('data-slot="provider-connection-card"');
    expect(finishMarkup).toContain(
      'aria-label="TouchGrassBar menu bar preview"',
    );
    expect(finishMarkup).toContain(
      'data-icon-source="macos-native-cursor-arrow"',
    );
    expect(finishMarkup).toContain(
      'data-icon-source="macos-sf-battery-charging"',
    );
    expect(finishMarkup).toContain('data-icon-source="macos-sf-wifi"');
    expect(finishMarkup).toContain('data-icon-source="macos-sf-search"');
    expect(finishMarkup).not.toContain('data-icon-source="KeyboardIcon"');
    expect(finishMarkup).toContain("menu-bar-app-target");
    expect(finishMarkup).toContain('data-slot="menu-bar-headroom-icon"');
    expect(finishMarkup).toContain('data-slot="menu-bar-headroom-meter"');
    expect(finishMarkup).toContain('data-slot="menu-bar-headroom-fill"');
    expect(finishMarkup).toContain('data-preview-value="illustrative"');
    expect(finishMarkup).toContain('data-meter-shape="rounded-pill"');
    expect(finishMarkup).toContain('data-native-meter-geometry="332x48"');
    expect(finishMarkup).toContain("h-[3.6px] w-[24.9px]");
    expect(finishMarkup).toContain("Tue 4 Aug");
    expect(markup.match(/>Unavailable</g)).toHaveLength(2);
    expect(markup).not.toContain(">Ready<");
    expect(markup).not.toContain(">Not installed<");
    expect(profileMarkup).toContain('aria-label="Display name"');
    expect(profileMarkup).toContain("Version 1.3.2");
    expect(profileMarkup).toMatch(/<form[^>]*>/);
    expect(profileMarkup).toMatch(/<button[^>]*type="submit"[^>]*>Continue<\/button>/);
    expect(profileMarkup).not.toContain("Fabien");
    expect(profileMarkup).toContain(
      "Other people can see your Display Name, TouchGrass ID, and daily scores on the Doomerboard.",
    );
    expect(profileMarkup).toContain(
      "They cannot see your prompts, conversations, credentials, logs, or files.",
    );
    expect(profileMarkup).toContain('data-profile-recovery-state="planned"');
    expect(profileMarkup).toContain(
      'data-profile-recovery-layout="step-actions"',
    );
    expect(profileMarkup).not.toContain("Already have a Profile?");
    expect(profileMarkup).not.toContain(
      "Restore it with your TouchGrass ID and Recovery Key.",
    );
    expect(profileMarkup).toMatch(
      /<button[^>]*disabled=""[^>]*>I have a Profile<\/button>/,
    );
    expect(profileMarkup).not.toContain("Coming soon");
    expect(profileMarkup).not.toContain('type="checkbox"');
    expect(unavailableFinishMarkup).toContain('data-setup-state="unavailable"');
    expect(unavailableFinishMarkup).toContain("Setup is not connected yet");
    expect(finishMarkup).not.toContain("Local setup ready");
    expect(finishMarkup).toContain("Profile Pending");
    expect(finishMarkup).not.toContain(">—<");
    expect(finishMarkup).toContain("retries automatically");
    expect(finishMarkup).toContain("local provider utility stays available");
    expect(finishMarkup).not.toContain("Recovery Key");
    expect(finishMarkup).not.toContain("Reveal key");
    expect(finishMarkup).not.toContain("Restore it");
    expect(finishMarkup).not.toContain("Recover on this Mac");
    expect(finishMarkup).not.toContain("TG-RK-");
    expect(pendingFinishMarkup).toMatch(
      /<button[^>]*>Retry Profile creation<\/button>/,
    );
    expect(submittingFinishMarkup).toContain("Creating your Profile…");
    expect(markup).toMatch(/<button[^>]*>Continue<\/button>/);
    expect(markup).not.toContain("Confirm your providers.");
  });

  test("accepts explicit development-only presentation values", () => {
    const profileMarkup = renderToStaticMarkup(
      <OnboardingScreen
        initialDisplayName="Fabien"
        initialStep="profile"
        onStartRecovery={() => undefined}
      />,
    );
    const finishMarkup = renderToStaticMarkup(
      <OnboardingScreen initialStep="finish" setupReady />,
    );

    expect(profileMarkup).toContain('value="Fabien"');
    expect(profileMarkup).toMatch(
      /<button(?![^>]*disabled="")[^>]*>I have a Profile<\/button>/,
    );
    expect(finishMarkup).toContain('data-setup-state="ready"');
    expect(finishMarkup).toContain("Local setup ready");
  });

  test("renders providers in the order supplied by Rust", () => {
    const markup = renderToStaticMarkup(
      <OnboardingScreen providers={[providers[1]]} />,
    );

    expect(markup).toContain(">Claude<");
    expect(markup).not.toContain(">Codex<");
    expect(markup.match(/data-slot="provider-connection-card"/g)).toHaveLength(
      1,
    );
  });
});
