const devAccents = ["blue", "violet", "rose", "amber", "teal"] as const;

const devAccentColors = {
  amber: "#f59e0b",
  blue: "#3b82f6",
  rose: "#f43f5e",
  teal: "#14b8a6",
  violet: "#8b5cf6",
} as const;

type DevAccent = (typeof devAccents)[number];

type DevInstance = {
  accent: DevAccent;
  identifier: string;
  key: string;
  label: string;
  port: number;
  productName: string;
  tag: string;
};

type ResolveDevInstanceInput = {
  accent?: string | undefined;
  branch: string;
  label?: string | undefined;
  port?: number | undefined;
  worktreeSeed: string;
};

const labelLimit = 36;
const portRangeStart = 15_000;
const portRangeSize = 1_000;

function stableHash(value: string) {
  let hash = 2_166_136_261;
  for (const character of value) {
    hash ^= character.codePointAt(0) ?? 0;
    hash = Math.imul(hash, 16_777_619);
  }
  return hash >>> 0;
}

function boundedLabel(value: string) {
  const printable = [...value]
    .map((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return codePoint <= 31 || codePoint === 127 ? " " : character;
    })
    .join("");
  const normalized = printable
    .replace(/\s+/g, " ")
    .trim();
  return [...normalized].slice(0, labelLimit).join("").trim();
}

function titleFromSlug(value: string) {
  const words = value
    .replace(/[^a-zA-Z0-9]+/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
  if (!words) return "Development";
  return `${words.charAt(0).toUpperCase()}${words.slice(1)}`;
}

function branchPresentation(branch: string, key: string) {
  const issue = /(?:^|\/)issue-(\d+)-(.+)$/.exec(branch);
  if (issue) {
    const tag = `#${issue[1]}`;
    const title = titleFromSlug(issue[2] ?? "");
    return { label: boundedLabel(`${tag} ${title}`), tag };
  }

  const title = titleFromSlug(branch.split("/").at(-1) ?? branch);
  const tag = key.slice(0, 4).toUpperCase();
  return { label: boundedLabel(`${title} · ${tag}`), tag };
}

function resolveDevInstance({
  accent,
  branch,
  label,
  port,
  worktreeSeed,
}: ResolveDevInstanceInput): DevInstance {
  const hash = stableHash(worktreeSeed);
  const key = hash.toString(36).padStart(7, "0");
  const presentation = branchPresentation(branch, key);
  const requestedLabel = label === undefined ? "" : boundedLabel(label);
  const resolvedAccent = devAccents.includes(accent as DevAccent)
    ? (accent as DevAccent)
    : devAccents[hash % devAccents.length]!;
  const resolvedPort =
    Number.isInteger(port) && (port ?? 0) >= 1_024 && (port ?? 0) <= 65_535
      ? (port as number)
      : portRangeStart + (hash % portRangeSize);

  return {
    accent: resolvedAccent,
    identifier: `app.touchgrass.bar.dev.w${key}`,
    key,
    label: requestedLabel || presentation.label,
    port: resolvedPort,
    productName: `TouchGrassBar Dev ${presentation.tag} ${key.slice(0, 4)}`,
    tag: presentation.tag,
  };
}

function parseDevInstance(value: string | undefined): DevInstance | null {
  if (!value) return null;
  try {
    const candidate = JSON.parse(value) as Partial<DevInstance>;
    if (
      typeof candidate.key !== "string" ||
      typeof candidate.label !== "string" ||
      typeof candidate.tag !== "string" ||
      typeof candidate.identifier !== "string" ||
      typeof candidate.productName !== "string" ||
      typeof candidate.port !== "number" ||
      !devAccents.includes(candidate.accent as DevAccent)
    ) {
      return null;
    }
    return candidate as DevInstance;
  } catch {
    return null;
  }
}

function currentDevInstance() {
  return parseDevInstance(import.meta.env.VITE_TOUCHGRASS_DEV_INSTANCE);
}

export {
  currentDevInstance,
  devAccentColors,
  devAccents,
  parseDevInstance,
  resolveDevInstance,
};
export type { DevAccent, DevInstance, ResolveDevInstanceInput };
