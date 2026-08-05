const devAccents = ["blue", "violet", "rose", "amber", "teal"] as const;

const devAccentColors = {
  amber: "#b45309",
  blue: "#1d4ed8",
  rose: "#be123c",
  teal: "#0f766e",
  violet: "#6d28d9",
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
  worktreeSeed: string;
};

const labelLimit = 36;
const tagLimit = 8;
const portRangeStart = 15_000;
const portRangeSize = 1_000;

function stableHash(value: string) {
  let hash = 14_695_981_039_346_656_037n;
  for (const character of value) {
    hash ^= BigInt(character.codePointAt(0) ?? 0);
    hash = BigInt.asUintN(64, hash * 1_099_511_628_211n);
  }
  return hash;
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
    const tag = boundedLabel(`#${issue[1]}`).slice(0, tagLimit);
    const title = titleFromSlug(issue[2] ?? "");
    return { label: boundedLabel(`${tag} ${title}`), tag };
  }

  const title = titleFromSlug(branch.split("/").at(-1) ?? branch);
  const tag = key.slice(0, 4).toUpperCase();
  return { label: boundedLabel(`${title} · ${tag}`), tag };
}

function labelTag(label: string, fallback: string) {
  if (!label) return fallback;
  const firstWord = /[#a-zA-Z0-9]+/.exec(label)?.[0] ?? fallback;
  return firstWord.slice(0, tagLimit).toUpperCase();
}

function resolveDevInstance({
  accent,
  branch,
  label,
  worktreeSeed,
}: ResolveDevInstanceInput): DevInstance {
  const hash = stableHash(worktreeSeed);
  const key = hash.toString(36).padStart(7, "0");
  const presentation = branchPresentation(branch, key);
  const requestedLabel = label === undefined ? "" : boundedLabel(label);
  const tag = labelTag(requestedLabel, presentation.tag);
  const resolvedAccent = devAccents.includes(accent as DevAccent)
    ? (accent as DevAccent)
    : devAccents[Number(hash % BigInt(devAccents.length))]!;
  const resolvedPort = portRangeStart + Number(hash % BigInt(portRangeSize));

  return {
    accent: resolvedAccent,
    identifier: `app.touchgrass.bar.dev.w${key}`,
    key,
    label: requestedLabel || presentation.label,
    port: resolvedPort,
    productName: `TouchGrassBar Dev ${tag} ${key.slice(0, 4)}`,
    tag,
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
