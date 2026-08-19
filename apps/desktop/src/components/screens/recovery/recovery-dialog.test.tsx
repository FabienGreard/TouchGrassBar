// @vitest-environment happy-dom

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { Window } from "happy-dom";
import { afterEach, expect, test, vi } from "vitest";

import { RecoveryDialog } from "./recovery-dialog";

afterEach(cleanup);

test("the branded recovery dialog sends the exact trimmed credentials", async () => {
  expect(typeof Window).toBe("function");
  const onOpenChange = vi.fn();
  const onRecover = vi.fn().mockResolvedValue(true);
  render(
    <RecoveryDialog
      onOpenChange={onOpenChange}
      onRecover={onRecover}
      open
    />,
  );

  expect(
    screen.getByRole("heading", { name: "Recover from another Mac" }),
  ).toBeDefined();
  fireEvent.change(screen.getByLabelText("TouchGrass ID"), {
    target: { value: "  TG-ABC234  " },
  });
  fireEvent.change(screen.getByLabelText(/Recovery Key/), {
    target: { value: `  ${"R".repeat(48)}  ` },
  });
  fireEvent.click(screen.getByRole("button", { name: "Recover on this Mac" }));

  await waitFor(() => {
    expect(onRecover).toHaveBeenCalledWith({
      recoveryKey: "R".repeat(48),
      touchGrassId: "TG-ABC234",
    });
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});

test("missing credentials reach the generic recovery failure path", async () => {
  const onRecover = vi.fn().mockResolvedValue(false);
  render(
    <RecoveryDialog onOpenChange={vi.fn()} onRecover={onRecover} open />,
  );

  fireEvent.click(screen.getByRole("button", { name: "Recover on this Mac" }));

  await waitFor(() => {
    expect(onRecover).toHaveBeenCalledWith({
      recoveryKey: "",
      touchGrassId: "",
    });
    expect(screen.getByRole("status").textContent).toContain(
      "Profile recovery unavailable",
    );
  });
});

test("the dialog cannot be dismissed while recovery is running", async () => {
  let finishRecovery: ((recovered: boolean) => void) | undefined;
  const onOpenChange = vi.fn();
  const onRecover = vi.fn(
    () =>
      new Promise<boolean>((resolve) => {
        finishRecovery = resolve;
      }),
  );
  render(
    <RecoveryDialog
      onOpenChange={onOpenChange}
      onRecover={onRecover}
      open
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Recover on this Mac" }));
  await waitFor(() => expect(onRecover).toHaveBeenCalledOnce());

  fireEvent.keyDown(document, { key: "Escape" });
  const overlay = document.querySelector('[data-slot="dialog-overlay"]');
  if (!overlay) throw new Error("Recovery dialog overlay is missing");
  fireEvent.pointerDown(overlay);
  expect(onOpenChange).not.toHaveBeenCalled();

  finishRecovery?.(true);
  await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
});
