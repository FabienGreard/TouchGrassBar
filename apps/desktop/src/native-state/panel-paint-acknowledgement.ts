import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  PANEL_PAINT_REQUEST_EVENT,
  type PanelPaintRequest,
} from "@touchgrass/contracts";

type PanelPaintEvent = { payload: PanelPaintRequest };
type PanelPaintBindings = {
  acknowledge: (sequence: number) => Promise<void>;
  cancelAnimationFrame: (id: number) => void;
  listen: (
    event: string,
    receive: (event: PanelPaintEvent) => void,
  ) => Promise<() => void>;
  prepare: () => Promise<void>;
  requestAnimationFrame: (callback: FrameRequestCallback) => number;
  takeRequest: () => Promise<PanelPaintRequest | null>;
};

const noop = () => undefined;

let resizeRevision = 0;
let latestResize = {
  failed: false,
  revision: resizeRevision,
  settled: Promise.resolve(),
};

function trackPanelNativeResize(operation: Promise<unknown>) {
  resizeRevision += 1;
  const state = {
    failed: false,
    revision: resizeRevision,
    settled: Promise.resolve(),
  };
  state.settled = operation.then(
    () => undefined,
    () => {
      state.failed = true;
    },
  );
  latestResize = state;
}

async function waitForPanelNativeResize() {
  const state = latestResize;
  await state.settled;
  if (state !== latestResize) return waitForPanelNativeResize();
  if (state.failed) throw new Error("Panel resize failed.");
}

async function preparePanelForPaint() {
  await document.fonts?.ready;
  await Promise.all(
    Array.from(document.images, (image) =>
      typeof image.decode === "function"
        ? image.decode().catch(() => undefined)
        : Promise.resolve(),
    ),
  );
  await waitForPanelNativeResize();
}

const defaultBindings: PanelPaintBindings = {
  acknowledge: (sequence) =>
    invoke("acknowledge_panel_paint", { sequence }).then(() => undefined),
  cancelAnimationFrame: (id) => window.cancelAnimationFrame(id),
  listen: (event, receive) => listen<PanelPaintRequest>(event, receive),
  prepare: preparePanelForPaint,
  requestAnimationFrame: (callback) => window.requestAnimationFrame(callback),
  takeRequest: () =>
    invoke<PanelPaintRequest | null>("take_panel_paint_request"),
};

async function activatePanelPaintAcknowledgement(
  bindings: PanelPaintBindings = defaultBindings,
): Promise<() => void> {
  let active = true;
  let firstFrame: number | undefined;
  let secondFrame: number | undefined;
  let latestSequence = 0;

  const cancelFrames = () => {
    if (firstFrame !== undefined)
      bindings.cancelAnimationFrame(firstFrame);
    if (secondFrame !== undefined)
      bindings.cancelAnimationFrame(secondFrame);
    firstFrame = undefined;
    secondFrame = undefined;
  };
  const schedule = ({ sequence }: PanelPaintRequest) => {
    if (!active || sequence <= latestSequence) return;
    latestSequence = sequence;
    cancelFrames();
    void bindings
      .prepare()
      .then(() => {
        if (!active || sequence !== latestSequence) return;
        firstFrame = bindings.requestAnimationFrame(() => {
          firstFrame = undefined;
          secondFrame = bindings.requestAnimationFrame(() => {
            secondFrame = undefined;
            void bindings
              .prepare()
              .then(() => {
                if (active && sequence === latestSequence) {
                  void bindings.acknowledge(sequence).catch(() => undefined);
                }
              })
              .catch(() => undefined);
          });
        });
      })
      .catch(() => undefined);
  };

  let stopListening: () => void = noop;
  try {
    stopListening = await bindings.listen(PANEL_PAINT_REQUEST_EVENT, (event) =>
      schedule(event.payload),
    );
  } catch {
    // The pending native request is still drained below.
  }

  try {
    const pending = await bindings.takeRequest();
    if (pending) schedule(pending);
  } catch {
    // A later native event can still deliver the request.
  }

  return () => {
    active = false;
    cancelFrames();
    stopListening();
  };
}

export { activatePanelPaintAcknowledgement, trackPanelNativeResize };
export type { PanelPaintBindings };
