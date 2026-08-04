import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { SanitizedDesktopState } from "@touchgrass/contracts";
import {
  REVISION_NOTICE_EVENT,
  revisionNoticeSchema,
  sanitizedDesktopStateSchema,
} from "@touchgrass/contracts";
import { useCallback, useEffect, useRef, useState } from "react";

import {
  acceptNewerSnapshot,
  browserFixture,
  resolveBrowserFixtureName,
  shouldHidePanel,
} from "../../nativeState";
import {
  currentDoomerboardPreviewRows,
  currentUsagePreview,
  friendsDoomerboardPreviewRows,
} from "../../previewFixtures";
import { PanelView } from "./panel-view";

function PanelScreen() {
  const [state, setState] = useState<SanitizedDesktopState | null>(null);
  const [error, setError] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const requestInFlight = useRef<Promise<void> | null>(null);
  const hasNativeRuntime = "__TAURI_INTERNALS__" in window;
  const previewFixtureName = hasNativeRuntime
    ? "unavailable"
    : resolveBrowserFixtureName(window.location.search);
  const hasCurrentPreview =
    previewFixtureName === "current" || previewFixtureName === "update";

  const readSnapshot = useCallback(() => {
    if (requestInFlight.current) return requestInFlight.current;
    const payloadRequest =
      import.meta.env.DEV && !hasNativeRuntime
        ? Promise.resolve(
            previewFixtureName === "loading"
              ? null
              : browserFixture(previewFixtureName),
          )
        : invoke<unknown>("get_sanitized_state");
    const request = payloadRequest
      .then((rawPayload) => {
        if (rawPayload === null && previewFixtureName === "loading") {
          setState(null);
          setError(false);
          return;
        }
        const candidate = sanitizedDesktopStateSchema.parse(rawPayload);
        setState((current) => acceptNewerSnapshot(current, candidate));
        setError(false);
      })
      .catch(() => setError(true))
      .finally(() => {
        requestInFlight.current = null;
      });
    requestInFlight.current = request;
    return request;
  }, [hasNativeRuntime, previewFixtureName]);

  useEffect(() => {
    let active = true;
    const subscription = hasNativeRuntime
      ? listen<unknown>(REVISION_NOTICE_EVENT, (event) => {
          if (!active || !revisionNoticeSchema.safeParse(event.payload).success)
            return;
          void readSnapshot();
        })
      : Promise.resolve(() => undefined);

    void subscription.then((unlisten) => {
      if (!active) unlisten();
      else void readSnapshot();
    });

    const onKeyDown = (event: KeyboardEvent) => {
      if (hasNativeRuntime && shouldHidePanel(event)) void invoke("hide_panel");
      if (hasNativeRuntime && event.metaKey && event.key === ",")
        void invoke("open_settings");
    };
    window.addEventListener("keydown", onKeyDown);

    return () => {
      active = false;
      window.removeEventListener("keydown", onKeyDown);
      void subscription.then((unlisten) => unlisten());
    };
  }, [hasNativeRuntime, readSnapshot]);

  useEffect(() => {
    if (!hasNativeRuntime || typeof ResizeObserver === "undefined") return;

    const panel = document.querySelector<HTMLElement>(
      '[data-slot="panel-shell"]',
    );
    if (!panel) return;

    let lastHeight = 0;
    const observer = new ResizeObserver(() => {
      const height = Math.ceil(panel.getBoundingClientRect().height);
      if (height === lastHeight) return;
      lastHeight = height;
      void invoke("resize_panel", { height });
    });
    observer.observe(panel);

    return () => observer.disconnect();
  }, [hasNativeRuntime]);

  const refresh = () => {
    setRefreshing(true);
    const request = hasNativeRuntime
      ? invoke("request_refresh")
      : Promise.resolve();
    void request.then(() => readSnapshot()).finally(() => setRefreshing(false));
  };

  return (
    <PanelView
      doomerboardPreviewRows={
        import.meta.env.DEV &&
        !hasNativeRuntime &&
        hasCurrentPreview
          ? currentDoomerboardPreviewRows
          : undefined
      }
      error={error}
      nativeGlass
      onRefresh={refresh}
      onSettings={() => {
        if (hasNativeRuntime) void invoke("open_settings");
      }}
      onUpdate={() => undefined}
      refreshing={refreshing}
      state={state}
      tokenmaxxerPreviewRows={
        import.meta.env.DEV &&
        !hasNativeRuntime &&
        hasCurrentPreview
          ? friendsDoomerboardPreviewRows
          : undefined
      }
      usagePreview={
        import.meta.env.DEV &&
        !hasNativeRuntime &&
        hasCurrentPreview
          ? currentUsagePreview
          : undefined
      }
      updateAvailable={previewFixtureName === "update"}
    />
  );
}

export { PanelScreen };
