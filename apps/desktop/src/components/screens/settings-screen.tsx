import { invoke } from "@tauri-apps/api/core";
import {
  NativeWindow,
  NativeWindowContent,
  NativeWindowNav,
  NativeWindowNavItem,
  Switch,
} from "@touchgrass/ui";
import { useEffect, useState } from "react";

import { ScreenSidebar } from "@/components/screens/screen-sidebar";

function SettingsScreen() {
  const [launchAtLogin, setLaunchAtLogin] = useState<boolean | null>(null);

  useEffect(() => {
    void invoke<boolean>("launch_at_login_enabled")
      .then(setLaunchAtLogin)
      .catch(() => setLaunchAtLogin(null));
  }, []);

  const setLaunchSetting = (enabled: boolean) => {
    void invoke("set_launch_at_login", { enabled })
      .then(() => setLaunchAtLogin(enabled))
      .catch(() => setLaunchAtLogin((current) => current));
  };

  return (
    <NativeWindow>
      <ScreenSidebar footer="Private work stays on this Mac." title="Settings">
        <NativeWindowNav aria-label="Settings sections">
          <NativeWindowNavItem aria-current="page">General</NativeWindowNavItem>
          <NativeWindowNavItem aria-disabled="true">
            Providers
          </NativeWindowNavItem>
          <NativeWindowNavItem aria-disabled="true">
            Identity
          </NativeWindowNavItem>
          <NativeWindowNavItem aria-disabled="true">
            Updates
          </NativeWindowNavItem>
        </NativeWindowNav>
      </ScreenSidebar>

      <NativeWindowContent>
        <div className="mx-auto w-full max-w-[660px]">
          <small className="font-mono text-[10px] font-semibold tracking-[0.12em] text-sheet-green uppercase">
          Settings
          </small>
          <h1 className="mt-2.5 mb-2 text-[34px] leading-none tracking-[-0.045em]">
            General
          </h1>
          <p className="mt-0 text-[13px] leading-6 text-sheet-muted">
            Choose how TouchGrassBar behaves on this Mac.
          </p>

        <section className="mt-7 overflow-hidden rounded-[12px] border border-sheet-row-border bg-sheet-row shadow-surface">
          <label
            className="flex cursor-pointer items-center justify-between gap-8 p-4.5"
            htmlFor="launch-at-login"
          >
            <span>
              <strong className="block text-[13px]">Launch at login</strong>
              <small className="mt-1 block max-w-[330px] text-[11px] leading-4 text-sheet-muted">
                {launchAtLogin === null
                  ? "Reading the macOS setting…"
                  : launchAtLogin
                    ? "TouchGrassBar starts with this Mac."
                    : "TouchGrassBar starts only when you open it."}
              </small>
            </span>
              <Switch
              aria-label="Launch TouchGrassBar at login"
              checked={launchAtLogin ?? false}
              disabled={launchAtLogin === null}
              id="launch-at-login"
              onCheckedChange={setLaunchSetting}
              />
          </label>
        </section>

        <section className="mt-4 rounded-[12px] border border-sheet-line bg-white/35 p-4.5">
          <strong className="block text-[12px]">Local by design</strong>
          <p className="mt-1.5 mb-0 text-[11px] leading-5 text-sheet-muted">
            Provider access remains native. Prompts, conversations, credentials,
            and raw session content are never exposed to this interface.
          </p>
        </section>
        </div>
      </NativeWindowContent>
    </NativeWindow>
  );
}

export { SettingsScreen };
