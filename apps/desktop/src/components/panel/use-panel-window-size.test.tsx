// @vitest-environment happy-dom

import { invoke } from "@tauri-apps/api/core";
import { cleanup, render } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { usePanelWindowSize } from "./use-panel-window-size";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function Panel() {
  usePanelWindowSize(true);
  return (
    <div data-slot="panel-viewport">
      <main data-slot="panel-shell" />
    </div>
  );
}

test("requests the full content height even when the window is shorter", () => {
  let notify: () => void = vi.fn();
  const disconnect = vi.fn();
  const observe = vi.fn();
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        notify = callback;
      }
      observe = observe;
      disconnect = disconnect;
    },
  );
  let contentHeight = 764.75;
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
    function (this: HTMLElement) {
      return { height: this.dataset.slot === "panel-shell" ? contentHeight : 640 } as DOMRect;
    },
  );

  const view = render(<Panel />);
  expect(observe).toHaveBeenCalledWith(view.container.querySelector('[data-slot="panel-shell"]'));
  notify();
  expect(invoke).toHaveBeenLastCalledWith("resize_panel", { height: 765 });
  notify();
  expect(invoke).toHaveBeenCalledTimes(1);
  contentHeight = 800.25;
  notify();
  expect(invoke).toHaveBeenLastCalledWith("resize_panel", { height: 801 });
  view.unmount();
  expect(disconnect).toHaveBeenCalledOnce();
});
