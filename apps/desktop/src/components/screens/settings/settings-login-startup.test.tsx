// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { SettingsCoordinator } from "./settings-coordinator";

const native = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: native.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => undefined }));

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

test("opens Apple settings only on request, then refreshes the switch on return", async () => {
  let approved = false;
  native.invoke.mockImplementation(async (command: string) => {
    if (command === "get_settings_state") {
      return {
        contractVersion: 5,
        launchAtLogin: approved
          ? { availability: "available", enabled: true }
          : { availability: "requiresApproval" },
        profileProvisioning: "not-authorized",
        providers: [],
        section: "general",
      };
    }
    if (command === "get_update_state") throw new Error("unavailable");
    return undefined;
  });

  const { unmount } = render(<SettingsCoordinator />);
  await screen.findByText("macOS approval required.");
  expect(screen.getByRole("switch", { name: "Open at login" }).getAttribute("aria-checked")).toBe(
    "false",
  );
  expect(native.invoke).not.toHaveBeenCalledWith("open_login_items_settings", undefined);

  fireEvent.click(screen.getByRole("button", { name: "Open System Settings ↗" }));
  await waitFor(() =>
    expect(native.invoke).toHaveBeenCalledWith("open_login_items_settings", undefined),
  );
  expect(screen.queryByText("macOS approval required.")).not.toBeNull();

  approved = true;
  fireEvent.focus(window);
  await waitFor(() =>
    expect(screen.getByRole("switch", { name: "Open at login" }).getAttribute("aria-checked")).toBe(
      "true",
    ),
  );
  expect(screen.queryByText("macOS approval required.")).toBeNull();
  expect(screen.queryByRole("button", { name: "Open System Settings ↗" })).toBeNull();
  expect(native.invoke).not.toHaveBeenCalledWith("set_launch_at_login", expect.anything());

  unmount();
  native.invoke.mockClear();
  fireEvent.focus(window);
  expect(native.invoke).not.toHaveBeenCalled();
});

test("uses the button label for failure feedback and permits a retry", async () => {
  native.invoke.mockImplementation(async (command: string) => {
    if (command === "get_settings_state") {
      return {
        contractVersion: 5,
        launchAtLogin: { availability: "requiresApproval" },
        profileProvisioning: "not-authorized",
        providers: [],
        section: "general",
      };
    }
    throw new Error("private native path");
  });
  render(<SettingsCoordinator />);
  fireEvent.click(await screen.findByRole("button", { name: "Open System Settings ↗" }));
  const retry = await screen.findByRole("button", { name: "Could not open. Try again" });
  expect(
    screen.queryByText("Could not open System Settings. Open General → Login Items manually."),
  ).toBeNull();
  expect(screen.queryByText("private native path")).toBeNull();
  native.invoke.mockResolvedValueOnce(undefined);
  fireEvent.click(retry);
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "Open System Settings ↗" }).hasAttribute("disabled"),
    ).toBe(false),
  );
});
