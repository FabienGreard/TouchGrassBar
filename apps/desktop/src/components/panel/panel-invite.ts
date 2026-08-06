import { listen } from "@tauri-apps/api/event";
import { PANEL_INVITE_EVENT } from "@touchgrass/contracts";

type StopListening = () => void;
type ListenForInvite = (
  event: string,
  receive: () => void,
) => Promise<StopListening>;

const listenForInvite: ListenForInvite = (event, receive) =>
  listen(event, receive);

async function subscribeToPanelInvite(
  receive: () => void,
  subscribe: ListenForInvite = listenForInvite,
): Promise<StopListening> {
  try {
    return await subscribe(PANEL_INVITE_EVENT, receive);
  } catch {
    return () => undefined;
  }
}

export { subscribeToPanelInvite };
