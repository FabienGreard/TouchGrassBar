import { Button } from "@touchgrass/ui";
import { useRef, useState } from "react";

function SupportSettings({
  readReport,
}: {
  readReport?: (() => Promise<string | null>) | undefined;
}) {
  const [status, setStatus] = useState<"idle" | "reading" | "copied" | "failed">("idle");
  const report = useRef<string | null>(null);

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
        Copy Claude scan status and daily cost estimates to share with support. The report contains
        no conversations, file paths, or credentials.
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
    </section>
  );
}

export { SupportSettings };
