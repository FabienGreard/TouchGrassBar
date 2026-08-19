import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { PANEL_ADD_TOKENMAXXER_EVENT } from "@touchgrass/contracts";

type StopListening = () => void;
type ListenForAddTokenmaxxer = (event: string, receive: () => void) => Promise<StopListening>;
type TakeAddTokenmaxxerRequest = () => Promise<boolean>;

const noop: StopListening = () => undefined;
const listenForAddTokenmaxxer: ListenForAddTokenmaxxer = (event, receive) => listen(event, receive);
const takeAddTokenmaxxerRequest: TakeAddTokenmaxxerRequest = () =>
  invoke("take_panel_add_tokenmaxxer_request");

async function subscribeToPanelAddTokenmaxxer(
  receive: () => void,
  subscribe: ListenForAddTokenmaxxer = listenForAddTokenmaxxer,
  takeRequest: TakeAddTokenmaxxerRequest = takeAddTokenmaxxerRequest,
): Promise<StopListening> {
  let active = true;
  let poll: ReturnType<typeof setInterval> | undefined;
  const drain = async () => {
    if (!active) return;
    try {
      if (await takeRequest()) receive();
    } catch {
      // The native request stays pending, so a later drain can retry it.
    }
  };

  let stop = noop;
  try {
    stop = await subscribe(PANEL_ADD_TOKENMAXXER_EVENT, () => void drain());
  } catch {
    poll = setInterval(() => void drain(), 250);
  }
  await drain();

  return () => {
    active = false;
    if (poll !== undefined) clearInterval(poll);
    stop();
  };
}

export { subscribeToPanelAddTokenmaxxer };
