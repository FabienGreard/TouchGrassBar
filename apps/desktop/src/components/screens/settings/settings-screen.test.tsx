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

    expect(markup).toContain("Profile unavailable");
    expect(markup).not.toContain("Fabien");
    expect(markup).not.toContain("#TG-7K4P9D");
    expect(markup).toContain("Profile security");
    expect(markup).toContain(
      "Recovery and key access require a secure macOS sheet and are not available in this build.",
    );
    expect(markup).not.toContain('type="password"');
    expect(markup).not.toContain("Recovery Key");
    expect(markup).not.toContain("Reveal recovery key");
    expect(markup).not.toContain("Enter Recovery Key");
    expect(markup).not.toContain("Recover on this Mac");
    expect(markup).not.toContain("TG-RK-");
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
});
