import { useCallback, useEffect, useState } from "react";

type CopyStatus = "copied" | "idle" | "unavailable";

function useCopyText(value: string) {
  const [copyStatus, setCopyStatus] = useState<CopyStatus>("idle");

  useEffect(() => {
    if (copyStatus === "idle") return undefined;
    const resetTimer = window.setTimeout(() => setCopyStatus("idle"), 1600);
    return () => window.clearTimeout(resetTimer);
  }, [copyStatus]);

  const copyText = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopyStatus("copied");
    } catch {
      setCopyStatus("unavailable");
    }
  }, [value]);

  return { copyStatus, copyText };
}

export { useCopyText };
export type { CopyStatus };
