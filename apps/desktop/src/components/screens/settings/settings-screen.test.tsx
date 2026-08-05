import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";

import { CodingProviderAccessCard } from "@/components/coding-provider-access";

import { ProfileSettings } from "./profile-settings";
import { SettingsScreen } from "./settings-screen";

describe("settings screen", () => {
  test("uses the approved native-sheet composition", () => {
    const markup = renderToStaticMarkup(<SettingsScreen />);

    expect(markup).toContain('data-slot="native-window"');
    expect(markup).toContain('aria-current="page"');
    expect(markup).toContain("General");
    expect(markup).toContain("Open at login");
    expect(markup).toContain("Providers");
    expect(markup).toContain("Profile");
    expect(markup).toContain("View on GitHub");
    expect(markup).toContain('data-slot="switch"');
    expect(markup).not.toContain("Preferences");
    expect(markup).not.toContain("Control the spiral.");
  });

  test("keeps Profile recovery secrets outside React", () => {
    const markup = renderToStaticMarkup(<ProfileSettings />);
    const developmentMarkup = renderToStaticMarkup(
      <ProfileSettings onStartRecovery={() => undefined} />,
    );

    expect(markup).toContain("Profile unavailable");
    expect(markup).not.toContain("Fabien");
    expect(markup).not.toContain("#TG-7K4P9D");
    expect(markup).toContain("Profile security");
    expect(markup).not.toContain("local macOS Keychain");
    expect(markup).not.toContain('type="password"');
    expect(markup).not.toContain("Reveal recovery key");
    expect(markup).toContain("Recover from another Mac");
    expect(markup).toMatch(
      /<button[^>]*disabled=""[^>]*>Enter Recovery Key…<\/button>/,
    );
    expect(markup).not.toContain("Recover on this Mac");
    expect(markup).not.toContain("TG-RK-");
    expect(developmentMarkup).toMatch(
      /<button[^>]*>Enter Recovery Key…<\/button>/,
    );
    expect(developmentMarkup).not.toContain('type="password"');
  });

  test("presents Profile Pending without inventing a public ID", () => {
    const markup = renderToStaticMarkup(
      <ProfileSettings
        pendingDisplayName="Fabien"
        profileProvisioning="profile-pending"
      />,
    );

    expect(markup).toContain('data-profile-state="profile-pending"');
    expect(markup).toContain("Profile Pending");
    expect(markup).toContain(">Fabien<");
    expect(markup).toContain(
      "Assigned automatically when Profile services are available.",
    );
    expect(markup).not.toContain("#TG-");
  });

  test("keeps disconnected production settings honest and inert", () => {
    const generalMarkup = renderToStaticMarkup(<SettingsScreen />);
    const providerMarkup = renderToStaticMarkup(
      <CodingProviderAccessCard provider="codex" state="unavailable" />,
    );

    expect(generalMarkup).toContain("Not connected in this build.");
    expect(generalMarkup.match(/role="switch"[^>]*disabled=""/g)).toHaveLength(
      2,
    );
    expect(generalMarkup).toMatch(/<button[^>]*disabled=""[^>]*>Check now/);
    expect(generalMarkup).toMatch(
      /<button[^>]*disabled=""[^>]*>View on GitHub/,
    );
    expect(providerMarkup).toContain("Unavailable");
    expect(providerMarkup).toContain(
      "Provider detection is not connected in this build.",
    );
    expect(providerMarkup).not.toContain("Check now");
    expect(providerMarkup).not.toContain("Check again");
  });

  test("names provider actions independently for VoiceOver", () => {
    const markup = renderToStaticMarkup(
      <SettingsScreen
        codexState="detected"
        onCheckProviders={() => undefined}
        providerState="not-installed"
        section="providers"
      />,
    );

    expect(markup).toContain('aria-label="Check Codex again"');
    expect(markup).toContain('aria-label="Check Claude again"');
  });
});
