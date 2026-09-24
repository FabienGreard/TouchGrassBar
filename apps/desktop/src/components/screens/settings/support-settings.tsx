import type { SupportSessionState } from "@touchgrass/contracts";
import { Button } from "@touchgrass/ui";
import { useEffect, useRef, useState } from "react";

function SupportSettings({
  readReport,
  readSession,
  setSession,
}: {
  readSession?: (() => Promise<SupportSessionState | null>) | undefined;
  setSession?: ((enabled: boolean) => Promise<SupportSessionState | null>) | undefined;
  readReport?: (() => Promise<string | null>) | undefined;
}) {
  const [status, setStatus] = useState<"idle" | "reading" | "copied" | "failed">("idle");
  const report = useRef<string | null>(null);
  const [session, updateSession] = useState<SupportSessionState | null>(null);
  const [sessionBusy, setSessionBusy] = useState(false);
  const [sessionFailed, setSessionFailed] = useState(false);
  const sessionOperation = useRef(0);
  useEffect(() => {
    if (!readSession) return;
    let stopped = false;
    const refresh = async () => {
      const operation = sessionOperation.current;
      const value = await readSession();
      if (!stopped && operation === sessionOperation.current) updateSession(value);
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 10_000);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [readSession]);
  async function changeSession() {
    if (!setSession || sessionBusy) return;
    sessionOperation.current += 1;
    setSessionBusy(true);
    setSessionFailed(false);
    try {
      const value = await setSession(session?.expiresAt == null);
      if (!value) {
        setSessionFailed(true);
        return;
      }
      updateSession(value);
    } catch {
      setSessionFailed(true);
    } finally {
      sessionOperation.current += 1;
      setSessionBusy(false);
    }
  }

  async function copyReport() {
    if (!readReport || status === "reading") return;
    setStatus("reading");
    try {
      // Keep a failed copy available for a second click with a fresh clipboard gesture.
      const text = report.current ?? (await readReport());
      if (text === null) throw new Error();
      report.current = text;
      await navigator.clipboard.writeText(text);
      report.current = null;
      setStatus("copied");
    } catch {
      setStatus("failed");
    }
  }

  return (
    <section className="border-t border-sheet-line pt-5">
      <h2 className="m-0 text-[14px]">Support</h2>
      <p className="mt-1 mb-4 text-[10px] leading-4 text-sheet-muted">
        Copy scan status for all providers and daily cost estimates to share with support. The
        report contains no conversations, file paths, or credentials.
      </p>
      <Button
        disabled={!readReport || status === "reading"}
        aria-busy={status === "reading" || undefined}
        onClick={() => void copyReport()}
        type="button"
        variant="ghost"
      >
        {status === "reading" ? "Reading report…" : "Copy support report"}
      </Button>
      <p aria-live="polite" className="mt-2 text-[10px] text-sheet-muted">
        {status === "copied"
          ? "Report copied."
          : status === "failed"
            ? "Could not copy the report. Try again."
            : ""}
      </p>
      <p className="mt-4 mb-3 text-[10px] leading-4 text-sheet-muted">
        Allow support to request fresh reports for all providers for 30 minutes. Reports include
        scan counts and daily cost estimates. You can end access at any time. Quitting the app ends
        access. Reports are kept for 7 days.
      </p>
      <Button
        type="button"
        variant="ghost"
        disabled={!setSession || sessionBusy || session === null}
        aria-busy={sessionBusy || undefined}
        onClick={() => void changeSession()}
      >
        {sessionBusy
          ? "Updating support access…"
          : session?.expiresAt != null
            ? "End remote support"
            : "Allow remote support for 30 minutes"}
      </Button>
      <p aria-live="polite" className="mt-2 text-[10px] text-sheet-muted">
        {sessionFailed
          ? "Could not change support access. Try again."
          : session?.expiresAt != null
            ? `Remote support is active until ${new Date(session.expiresAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}.`
            : "Remote support is off."}
      </p>
    </section>
  );
}

export { SupportSettings };
