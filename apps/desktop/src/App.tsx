import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { SanitizedDesktopState } from "@touchgrass/contracts";
import { sanitizedDesktopStateSchema } from "@touchgrass/contracts";
import { GrassIcon, MetricCard, PanelShell, StatusPill } from "@touchgrass/ui";
import { useEffect, useState } from "react";

const currentWindow = getCurrentWindow();
const tokenFormatter = new Intl.NumberFormat("en", {
  maximumFractionDigits: 1,
  notation: "compact",
});

function formatTokens(value: number) {
  return tokenFormatter.format(value);
}

function ProviderLine({ state }: { state: SanitizedDesktopState["providers"][number] }) {
  const lane = state.quotaLanes[0];
  const remaining = lane?.remaining;
  const allowance = lane?.allowance;
  const percentage =
    remaining !== null && remaining !== undefined && allowance
      ? Math.round((remaining / allowance) * 100)
      : null;

  return (
    <section className="rounded-xl border border-ash-700 bg-ash-900 p-3">
      <div className="flex items-center justify-between">
        <h2 className="m-0 capitalize">{state.provider}</h2>
        <span className="text-xs text-ash-400">{state.freshness}</span>
      </div>
      {lane ? (
        <>
          <div className="mt-3 h-2 overflow-hidden rounded-full bg-ash-700">
            <div
              className="h-full rounded-full bg-grass-400"
              style={{ width: `${percentage ?? 0}%` }}
            />
          </div>
          <div className="mt-2 flex justify-between text-xs text-ash-400">
            <span>{lane.label}</span>
            <span>{percentage === null ? "Unavailable" : `${percentage}% remaining`}</span>
          </div>
        </>
      ) : (
        <p className="mb-0 mt-2 text-sm text-ash-400">Provider data unavailable</p>
      )}
    </section>
  );
}

function Panel() {
  const [state, setState] = useState<SanitizedDesktopState | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        void invoke("hide_panel");
      }
    };

    window.addEventListener("keydown", onKeyDown);
    void invoke<unknown>("get_sanitized_state")
      .then((payload) => setState(sanitizedDesktopStateSchema.parse(payload)))
      .catch(() => setError("Local provider state is unavailable."));

    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const codexUsage = state?.usage.codex.today;

  return (
    <PanelShell>
      <div className="p-4">
        <header className="flex items-start justify-between">
          <div className="flex items-center gap-2">
            <GrassIcon className="h-6 w-6 text-grass-300" />
            <div>
              <h1 className="m-0 text-base font-semibold">TouchGrassBar</h1>
              <p className="m-0 text-xs text-ash-400">you’re still here</p>
            </div>
          </div>
          <StatusPill>{state?.sync.status ?? "loading"}</StatusPill>
        </header>

        {error ? <p className="mt-4 rounded-xl border border-ash-700 p-3 text-sm">{error}</p> : null}

        <div className="mt-4 space-y-2">
          {state?.providers.map((provider) => (
            <ProviderLine key={provider.provider} state={provider} />
          ))}
        </div>

        <div className="mt-3 grid grid-cols-3 gap-2">
          <MetricCard label="Today" value={formatTokens(codexUsage?.observedTokens ?? 0)} />
          <MetricCard label="7 days" value={formatTokens(state?.usage.codex.sevenDays.observedTokens ?? 0)} />
          <MetricCard label="30 days" value={formatTokens(state?.usage.codex.thirtyDays.observedTokens ?? 0)} />
        </div>

        <button
          className="mt-3 w-full rounded-xl border border-ash-700 bg-ash-900 px-3 py-3 text-left text-sm transition hover:border-ash-400"
          onClick={() => invoke("open_settings")}
          type="button"
        >
          Open Doomerboard <span className="float-right text-ash-400">⌘,</span>
        </button>
      </div>
    </PanelShell>
  );
}

function Settings() {
  const [launchAtLogin, setLaunchAtLogin] = useState<boolean | null>(null);

  useEffect(() => {
    void invoke<boolean>("launch_at_login_enabled")
      .then(setLaunchAtLogin)
      .catch(() => setLaunchAtLogin(null));
  }, []);

  const toggleLaunchAtLogin = () => {
    const nextValue = !(launchAtLogin ?? false);
    void invoke("set_launch_at_login", { enabled: nextValue }).then(() =>
      setLaunchAtLogin(nextValue),
    );
  };

  return (
    <main className="min-h-screen bg-ash-950 p-8 text-ash-100">
      <p className="font-mono text-xs uppercase tracking-[0.18em] text-grass-300">Settings</p>
      <h1 className="mt-2 text-3xl font-semibold">Control the spiral.</h1>
      <div className="mt-8 grid gap-3 sm:grid-cols-2">
        <MetricCard label="Identity" value="Not created" detail="A public TouchGrass ID will be generated." />
        <section className="rounded-xl border border-ash-700 bg-ash-900 p-3">
          <p className="m-0 text-xs font-medium uppercase tracking-[0.13em] text-ash-400">Launch at login</p>
          <button
            className="mt-2 rounded-full bg-grass-400 px-4 py-2 text-sm font-semibold text-grass-950"
            onClick={toggleLaunchAtLogin}
            type="button"
          >
            {launchAtLogin === null ? "Unavailable" : launchAtLogin ? "Enabled" : "Disabled"}
          </button>
        </section>
      </div>
      <section className="mt-6 rounded-2xl border border-ash-700 bg-ash-900 p-5">
        <h2 className="m-0 text-lg">The boundary</h2>
        <p className="mb-0 mt-2 max-w-xl leading-7 text-ash-400">
          Only UTC daily usage aggregates synchronize. Prompts, conversations, credentials, cookies,
          raw logs, and local paths never cross the native boundary.
        </p>
      </section>
    </main>
  );
}

function Onboarding() {
  return (
    <main className="grid min-h-screen place-items-center bg-ash-950 p-8 text-ash-100">
      <section className="max-w-lg text-center">
        <GrassIcon className="mx-auto h-12 w-12 text-grass-300" />
        <p className="mt-5 font-mono text-xs uppercase tracking-[0.18em] text-grass-300">Welcome, Tokenmaxxer</p>
        <h1 className="mt-3 text-4xl font-semibold tracking-tight">Everything is fine. Let’s count it.</h1>
        <p className="mt-4 leading-7 text-ash-400">TouchGrassBar detects Codex and Claude locally, then publishes only daily totals after creating your public identity.</p>
        <button className="mt-6 rounded-full bg-grass-400 px-5 py-3 font-semibold text-grass-950" type="button">Create my TouchGrass ID</button>
      </section>
    </main>
  );
}

export function App() {
  switch (currentWindow.label) {
    case "settings":
      return <Settings />;
    case "onboarding":
      return <Onboarding />;
    default:
      return <Panel />;
  }
}
