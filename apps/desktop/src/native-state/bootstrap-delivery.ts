import { bootstrapStateSchema, type BootstrapState } from "@touchgrass/contracts";

import type { ProfileRecoveryCredentials } from "@/components/dialogs/recovery-dialog";

type BootstrapPortFaultCode =
  | "bootstrap-completion-unavailable"
  | "bootstrap-state-unavailable"
  | "profile-recovery-unavailable"
  | "surface-unavailable";

type BootstrapPortOutcome<Value> =
  | { ok: true; value: Value }
  | { fault: { code: BootstrapPortFaultCode }; ok: false };

type BootstrapPort = {
  complete: (displayName: string) => Promise<BootstrapPortOutcome<unknown>>;
  hide: () => Promise<BootstrapPortOutcome<void>>;
  read: () => Promise<BootstrapPortOutcome<unknown>>;
  recoverProfile: (credentials: ProfileRecoveryCredentials) => Promise<BootstrapPortOutcome<void>>;
};

type BootstrapDeliverySnapshot = {
  phase: "degraded" | "loading" | "ready";
  snapshot: BootstrapState | null;
  submitting: boolean;
};

function createBootstrapDelivery(port: BootstrapPort) {
  let current: BootstrapDeliverySnapshot = {
    phase: "loading",
    snapshot: null,
    submitting: false,
  };
  let readInFlight: Promise<void> | null = null;
  let submissionInFlight: Promise<boolean> | null = null;
  const listeners = new Set<() => void>();

  const publish = (next: BootstrapDeliverySnapshot) => {
    current = next;
    for (const listener of listeners) listener();
  };

  const accept = (value: unknown, submitting = false) => {
    const parsed = bootstrapStateSchema.safeParse(value);
    if (!parsed.success) {
      publish({ ...current, phase: "degraded", submitting });
      return false;
    }
    publish({ phase: "ready", snapshot: parsed.data, submitting });
    return true;
  };

  const read = () => {
    if (readInFlight !== null) return readInFlight;
    readInFlight = (async () => {
      const outcome = await port.read();
      if (!outcome.ok) {
        publish({ ...current, phase: "degraded", submitting: false });
        return;
      }
      accept(outcome.value);
    })().finally(() => {
      readInFlight = null;
    });
    return readInFlight;
  };

  return {
    complete(displayName: string) {
      if (submissionInFlight !== null) return submissionInFlight;
      const normalized = displayName.trim();
      if (normalized.length === 0 || [...normalized].length > 40) {
        publish({ ...current, phase: "degraded", submitting: false });
        return Promise.resolve(false);
      }
      publish({ ...current, submitting: true });
      submissionInFlight = (async () => {
        const outcome = await port.complete(normalized);
        if (!outcome.ok) {
          publish({ ...current, phase: "degraded", submitting: false });
          return false;
        }
        const accepted = accept(outcome.value);
        if (!accepted) return false;
        if (current.snapshot?.profileProvisioning !== "ready") return false;
        const hidden = await port.hide();
        if (!hidden.ok) {
          publish({ ...current, phase: "degraded", submitting: false });
          return false;
        }
        return true;
      })().finally(() => {
        submissionInFlight = null;
      });
      return submissionInFlight;
    },
    getSnapshot: () => current,
    hide: async () => {
      await port.hide();
    },
    read,
    recoverProfile(credentials: ProfileRecoveryCredentials) {
      if (submissionInFlight !== null) return submissionInFlight;
      publish({ ...current, submitting: true });
      submissionInFlight = (async () => {
        const recovered = await port.recoverProfile(credentials);
        if (!recovered.ok) {
          publish({ ...current, phase: "degraded", submitting: false });
          return false;
        }
        const state = await port.read();
        if (!state.ok || !accept(state.value)) return false;
        if (current.snapshot?.profileProvisioning !== "ready") return false;
        const hidden = await port.hide();
        if (!hidden.ok) {
          publish({ ...current, phase: "degraded", submitting: false });
          return false;
        }
        return true;
      })().finally(() => {
        submissionInFlight = null;
      });
      return submissionInFlight;
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export { createBootstrapDelivery };
export type {
  BootstrapDeliverySnapshot,
  BootstrapPort,
  BootstrapPortFaultCode,
  BootstrapPortOutcome,
};
