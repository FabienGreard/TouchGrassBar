import type { Doc } from "../_generated/dataModel";

export function profileHasUnclaimedAuthority(profile: Doc<"tokenmaxxers">) {
  return (
    profile.activeDeviceId === undefined &&
    (profile.activeAuthSessionId === undefined || profile.activeAuthSessionId === null) &&
    (profile.authSessionGeneration === undefined || profile.authSessionGeneration === 0)
  );
}
