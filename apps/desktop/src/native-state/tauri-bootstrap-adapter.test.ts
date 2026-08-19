import { describe, expect, test, vi } from "vitest";

import {
  createTauriBootstrapAdapter,
  type TauriBootstrapBindings,
} from "@/native-state/tauri-bootstrap-adapter";

describe("Tauri bootstrap adapter", () => {
  test("uses the closed bootstrap commands and exact completion payload", async () => {
    const bindings: TauriBootstrapBindings = {
      invoke: vi.fn(async (command) => ({ command })),
    };
    const adapter = createTauriBootstrapAdapter(bindings);

    await adapter.read();
    await adapter.complete("Fabien");
    await adapter.recoverProfile();
    await adapter.hide();

    expect(bindings.invoke).toHaveBeenNthCalledWith(
      1,
      "get_bootstrap_state",
      undefined,
    );
    expect(bindings.invoke).toHaveBeenNthCalledWith(2, "complete_bootstrap", {
      displayName: "Fabien",
    });
    expect(bindings.invoke).toHaveBeenNthCalledWith(
      3,
      "recover_profile",
      undefined,
    );
    expect(bindings.invoke).toHaveBeenNthCalledWith(
      4,
      "hide_surface",
      undefined,
    );
  });

  test("contains raw native failures behind bounded fault codes", async () => {
    const adapter = createTauriBootstrapAdapter({
      invoke: vi.fn(() => Promise.reject(new Error("private path detail"))),
    });

    expect(await adapter.read()).toEqual({
      fault: { code: "bootstrap-state-unavailable" },
      ok: false,
    });
    expect(await adapter.complete("Fabien")).toEqual({
      fault: { code: "bootstrap-completion-unavailable" },
      ok: false,
    });
    expect(await adapter.recoverProfile()).toEqual({
      fault: { code: "profile-recovery-unavailable" },
      ok: false,
    });
    expect(await adapter.hide()).toEqual({
      fault: { code: "surface-unavailable" },
      ok: false,
    });
  });
});
