export type MacosProcessRecord = Readonly<{
  cpuPercent: number;
  parentPid: number;
  pid: number;
  rssKilobytes: number;
}>;

export type MacosProcessTreeTotals = Readonly<{
  cpuPercent: number;
  rssBytes: number;
}>;

const positiveIntegerPattern = /^[1-9][0-9]*$/;
const nonnegativeIntegerPattern = /^(?:0|[1-9][0-9]*)$/;
const nonnegativeDecimalPattern = /^(?:0|[1-9][0-9]*)(?:\.[0-9]+)?$/;

function processRecordIsValid(record: MacosProcessRecord): boolean {
  return (
    Number.isSafeInteger(record.pid) &&
    record.pid > 0 &&
    Number.isSafeInteger(record.parentPid) &&
    record.parentPid >= 0 &&
    Number.isFinite(record.cpuPercent) &&
    record.cpuPercent >= 0 &&
    Number.isSafeInteger(record.rssKilobytes) &&
    record.rssKilobytes >= 0 &&
    Number.isSafeInteger(record.rssKilobytes * 1024)
  );
}

export function parseMacosProcessTable(
  output: string,
): MacosProcessRecord[] {
  const records: MacosProcessRecord[] = [];
  const processIdentifiers = new Set<number>();

  for (const line of output.split(/\r?\n/)) {
    const trimmedLine = line.trim();
    if (trimmedLine.length === 0) continue;

    const fields = trimmedLine.split(/\s+/);
    const [pidText, parentPidText, cpuText, rssText] = fields;
    if (
      fields.length !== 4 ||
      !pidText ||
      !parentPidText ||
      !cpuText ||
      !rssText ||
      !positiveIntegerPattern.test(pidText) ||
      !nonnegativeIntegerPattern.test(parentPidText) ||
      !nonnegativeDecimalPattern.test(cpuText) ||
      !nonnegativeIntegerPattern.test(rssText)
    ) {
      throw new Error("macOS process record is malformed.");
    }

    const record = {
      cpuPercent: Number(cpuText),
      parentPid: Number(parentPidText),
      pid: Number(pidText),
      rssKilobytes: Number(rssText),
    } satisfies MacosProcessRecord;
    if (!processRecordIsValid(record)) {
      throw new Error("macOS process record is malformed.");
    }
    if (processIdentifiers.has(record.pid)) {
      throw new Error("macOS process record has a duplicate PID.");
    }

    processIdentifiers.add(record.pid);
    records.push(record);
  }

  return records;
}

export function sumMacosProcessTree(
  records: readonly MacosProcessRecord[],
  rootPid: number,
): MacosProcessTreeTotals {
  if (!Number.isSafeInteger(rootPid) || rootPid <= 0) {
    throw new Error("Process-tree root PID is invalid.");
  }

  const recordsByPid = new Map<number, MacosProcessRecord>();
  const childPidsByParent = new Map<number, number[]>();
  for (const record of records) {
    if (!processRecordIsValid(record)) {
      throw new Error("macOS process record is malformed.");
    }
    if (recordsByPid.has(record.pid)) {
      throw new Error("macOS process record has a duplicate PID.");
    }
    recordsByPid.set(record.pid, record);
    const childPids = childPidsByParent.get(record.parentPid) ?? [];
    childPids.push(record.pid);
    childPidsByParent.set(record.parentPid, childPids);
  }

  if (!recordsByPid.has(rootPid)) {
    throw new Error("Process-tree root is absent.");
  }

  let cpuPercent = 0;
  let rssKilobytes = 0;
  const pendingPids = [rootPid];
  const selectedPids = new Set<number>();

  while (pendingPids.length > 0) {
    const pid = pendingPids.pop();
    if (pid === undefined || selectedPids.has(pid)) continue;
    const record = recordsByPid.get(pid);
    if (!record) continue;

    selectedPids.add(pid);
    cpuPercent += record.cpuPercent;
    rssKilobytes += record.rssKilobytes;
    pendingPids.push(...(childPidsByParent.get(pid) ?? []));
  }

  const rssBytes = rssKilobytes * 1024;
  if (!Number.isFinite(cpuPercent) || !Number.isSafeInteger(rssBytes)) {
    throw new Error("Process-tree total is invalid.");
  }

  return { cpuPercent, rssBytes };
}
