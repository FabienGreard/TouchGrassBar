import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";

import { DisplayNameInput } from "@/components/display-name-input";

test("display names do not use Mac text completion", () => {
  const markup = renderToStaticMarkup(
    <DisplayNameInput aria-label="Display name" value="Fabien" readOnly />,
  );

  expect(markup).toContain('autoCapitalize="off"');
  expect(markup).toContain('autoComplete="off"');
  expect(markup).toContain('autoCorrect="off"');
  expect(markup).toContain('spellCheck="false"');
});
