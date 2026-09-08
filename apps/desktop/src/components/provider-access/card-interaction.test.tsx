// @vitest-environment happy-dom

import { invoke } from "@tauri-apps/api/core";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

import { CodingProviderAccessCard } from "./card";
import { ProvidersStep } from "../screens/onboarding/providers-step";
import { SettingsScreen } from "../screens/settings/settings-screen";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));

const providers = [
  { displayName: "Codex", enabled: true, provider: "codex", state: "not-installed" },
  { displayName: "Claude", enabled: true, provider: "claude", state: "not-installed" },
] as const;

beforeEach(() => {
  vi.stubGlobal("__TAURI_INTERNALS__", {});
  vi.spyOn(window, "open").mockReturnValue(null);
  vi.mocked(invoke).mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

test.each(["setup", "settings"] as const)(
  "%s opens both provider guides through the native browser command",
  async (surface) => {
    render(
      surface === "setup" ? (
        <ProvidersStep providers={providers} />
      ) : (
        <SettingsScreen providers={providers} section="providers" />
      ),
    );

    for (const provider of providers) {
      const link = screen.getByRole("link", {
        name: `Open the official ${provider.displayName} installation guide`,
      });
      const usedBrowserNavigation = fireEvent.click(link);

      await waitFor(() => {
        expect(invoke).toHaveBeenCalledWith("open_provider_installation_guide", {
          provider: provider.provider,
        });
      });
      expect(usedBrowserNavigation).toBe(false);
    }
    expect(invoke).toHaveBeenCalledTimes(2);
  },
);

test("shows a browser-open failure and allows a successful retry", async () => {
  vi.mocked(invoke).mockRejectedValueOnce(new Error("native failure"));
  render(<CodingProviderAccessCard displayName="Claude" provider="claude" state="not-installed" />);
  const link = screen.getByRole("link", { name: "Open the official Claude installation guide" });

  fireEvent.click(link);
  expect((await screen.findByRole("alert")).textContent).toBe(
    "Could not open your browser. Try again.",
  );
  fireEvent.click(link);
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  expect(invoke).toHaveBeenCalledTimes(2);
});

test("keeps normal link navigation in the browser preview", () => {
  vi.unstubAllGlobals();
  render(<CodingProviderAccessCard displayName="Codex" provider="codex" state="not-installed" />);
  const link = screen.getByRole("link", { name: "Open the official Codex installation guide" });

  expect(link.getAttribute("href")).toBe("https://developers.openai.com/codex/cli/");
  expect(fireEvent.click(link)).toBe(true);
  expect(invoke).not.toHaveBeenCalled();
});
