import type { UpdateState } from "@touchgrass/contracts";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";

import { UpdateExperience } from "@/update/update-experience";

const available: UpdateState = {
  contractVersion: 1,
  currentVersion: "1.3.2",
  onlineFeaturesPaused: false,
  update: {
    presentation: "sheet",
    status: "available",
    version: "1.4.0",
  },
};

describe("update experience", () => {
  test("uses the approved non-modal sheet and keeps installation explicit", () => {
    const markup = renderToStaticMarkup(
      <UpdateExperience
        onDefer={() => undefined}
        onInstall={() => undefined}
        state={available}
        surface="panel"
      />,
    );

    expect(markup).toContain('data-slot="update-sheet"');
    expect(markup).toContain('role="status"');
    expect(markup).toContain("Fresh grass available.");
    expect(markup).toContain("TouchGrassBar 1.4.0 is ready.");
    expect(markup).toContain("Signature checked before install");
    expect(markup).toContain(">Later</button>");
    expect(markup).toContain(">Install &amp; Relaunch</button>");
    expect(markup).not.toContain('role="dialog"');
    expect(markup).not.toContain("silent install");
    expect(markup).not.toContain("rollback");
    expect(markup).not.toContain("downgrade");
  });

  test("Later presentation remains a keyboard and VoiceOver named row", () => {
    const markup = renderToStaticMarkup(
      <UpdateExperience
        onInstall={() => undefined}
        state={{
          ...available,
          update: {
            presentation: "row",
            status: "available",
            version: "1.4.0",
          },
        }}
        surface="panel"
      />,
    );

    expect(markup).toContain('data-slot="update-row"');
    expect(markup).toContain(
      'aria-label="TouchGrassBar 1.4.0 update available"',
    );
    expect(markup).toContain(
      'aria-label="Install TouchGrassBar 1.4.0 and relaunch"',
    );
    expect(markup).toContain("motion-reduce:transition-none");
    expect(markup).toContain("contrast-more:border-pearl-ink");

    const settingsMarkup = renderToStaticMarkup(
      <UpdateExperience
        onInstall={() => undefined}
        state={{
          ...available,
          update: {
            presentation: "row",
            status: "available",
            version: "1.4.0",
          },
        }}
        surface="settings"
      />,
    );
    expect(settingsMarkup).toContain('data-slot="update-row"');
    expect(settingsMarkup).not.toContain('data-slot="update-sheet"');
  });

  test("all required failures keep Retry and DMG recovery without raw detail", () => {
    for (const failure of [
      "network",
      "download",
      "signature",
      "interrupted",
      "low-disk",
      "permission",
      "replacement",
    ] as const) {
      const markup = renderToStaticMarkup(
        <UpdateExperience
          onOpenLatestDmg={() => undefined}
          onRetry={() => undefined}
          state={{
            ...available,
            update: { failure, status: "failed", version: "1.4.0" },
          }}
          surface="panel"
        />,
      );
      expect(markup).toContain('role="alert"');
      expect(markup).toContain(">Retry</button>");
      expect(markup).toContain(">Download latest DMG</button>");
      expect(markup).not.toContain("/private");
      expect(markup).not.toContain("latest.json");
    }
  });

  test("minimum-version copy pauses only online features", () => {
    const markup = renderToStaticMarkup(
      <UpdateExperience
        onDefer={() => undefined}
        onInstall={() => undefined}
        state={{ ...available, onlineFeaturesPaused: true }}
        surface="panel"
      />,
    );

    expect(markup).toContain('data-slot="online-feature-pause"');
    expect(markup).toContain("Update required for online features.");
    expect(markup).toContain(
      "Local provider data, Settings, and recovery remain available.",
    );
    expect(markup).toContain("Install &amp; Relaunch");
  });

  test("Settings gives a manual check independent from automatic cadence", () => {
    const markup = renderToStaticMarkup(
      <UpdateExperience
        onCheck={() => undefined}
        state={{ ...available, update: { status: "idle" } }}
        surface="settings"
      />,
    );

    expect(markup).toContain("Version 1.3.2");
    expect(markup).toContain(">Check for Updates</button>");
    expect(markup).toContain("at most once every 24 hours");
    expect(markup).toContain(
      "It never installs or restarts without your approval.",
    );
    expect(markup).not.toContain("provider refresh");

    const availableMarkup = renderToStaticMarkup(
      <UpdateExperience
        onCheck={() => undefined}
        onDefer={() => undefined}
        onInstall={() => undefined}
        state={available}
        surface="settings"
      />,
    );
    expect(availableMarkup).toContain(">Check for Updates</button>");
  });
});
