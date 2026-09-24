// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { SupportSettings } from "./support-settings";

afterEach(cleanup);

test("reads only on request and copies the report after the read", async () => {
  const readReport = vi.fn(async () => "sanitized report");
  const writeText = vi.fn(async () => undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  render(<SupportSettings readReport={readReport} />);
  expect(readReport).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Copy support report" }));
  await waitFor(() => expect(screen.getByText("Report copied.")).toBeTruthy());
  expect(writeText).toHaveBeenCalledWith("sanitized report");
  expect(readReport).toHaveBeenCalledTimes(1);
});

test("a failed clipboard write can retry with the same report", async () => {
  const readReport = vi.fn(async () => "sanitized report");
  const writeText = vi.fn().mockRejectedValueOnce(new Error("denied")).mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  render(<SupportSettings readReport={readReport} />);
  fireEvent.click(screen.getByRole("button", { name: "Copy support report" }));
  await waitFor(() =>
    expect(screen.getByText("Could not copy the report. Try again.")).toBeTruthy(),
  );
  fireEvent.click(screen.getByRole("button", { name: "Copy support report" }));
  await waitFor(() => expect(screen.getByText("Report copied.")).toBeTruthy());
  expect(readReport).toHaveBeenCalledTimes(1);
  expect(writeText).toHaveBeenCalledTimes(2);
});

test("an unavailable report is never copied", async () => {
  const writeText = vi.fn();
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  render(<SupportSettings readReport={async () => null} />);
  fireEvent.click(screen.getByRole("button", { name: "Copy support report" }));
  await waitFor(() =>
    expect(screen.getByText("Could not copy the report. Try again.")).toBeTruthy(),
  );
  expect(writeText).not.toHaveBeenCalled();
});
