// @vitest-environment happy-dom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";

import { Doomerboard } from "./doomerboard";

afterEach(cleanup);

test("copying the current Profile writes only its canonical TouchGrass ID", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText },
  });
  render(
    <Doomerboard
      currentProfile={{ displayName: "Fabien", touchGrassId: "#TG-7K4P9D" }}
      rows={[]}
    />,
  );

  fireEvent.click(screen.getByRole("button", { name: "Copy TouchGrass ID TG-7K4P9D" }));

  await waitFor(() => expect(writeText).toHaveBeenCalledWith("TG-7K4P9D"));
});
