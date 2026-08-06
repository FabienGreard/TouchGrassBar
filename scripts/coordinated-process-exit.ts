export type CoordinatedSignal = "SIGINT" | "SIGTERM";

const signalExitCodes: Record<CoordinatedSignal, number> = {
  SIGINT: 130,
  SIGTERM: 143,
};

export function coordinatedProcessExitCode(
  exitCode: number,
  signal: CoordinatedSignal | null,
): number {
  if (signal !== null && exitCode === signalExitCodes[signal]) return 0;
  return exitCode;
}
