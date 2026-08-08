export const downloadFallbackUrl =
  "https://github.com/FabienGreard/TouchGrassBar/releases/latest";
export const latestReleaseApiUrl =
  "https://api.github.com/repos/FabienGreard/TouchGrassBar/releases/latest";

const releaseRoot =
  "https://github.com/FabienGreard/TouchGrassBar/releases/download";
const stableTagPattern =
  /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;

type ApprovedDownload = {
  url: string;
  version: string;
};

type FetchResponse = {
  json(): Promise<unknown>;
  ok: boolean;
};

type FetchLatestRelease = (
  input: string,
  init: {
    credentials: "omit";
    headers: { Accept: string };
  },
) => Promise<FetchResponse>;

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : null;
}

export function approvedDownloadFromRelease(
  input: unknown,
): ApprovedDownload | null {
  const release = asRecord(input);
  if (
    !release ||
    release.draft !== false ||
    release.immutable !== true ||
    release.prerelease !== false ||
    typeof release.tag_name !== "string" ||
    !stableTagPattern.test(release.tag_name) ||
    typeof release.published_at !== "string" ||
    !Number.isFinite(Date.parse(release.published_at)) ||
    !Array.isArray(release.assets)
  ) {
    return null;
  }

  const version = release.tag_name.slice(1);
  const updaterArchive = `TouchGrassBar_${version}_aarch64.app.tar.gz`;
  const requiredAssetNames = [
    "latest.json",
    `release-trust-${version}.json`,
    "SHA256SUMS",
    `TouchGrassBar_${version}_aarch64.dmg`,
    updaterArchive,
    `${updaterArchive}.sig`,
  ];
  const approvedAssets = new Map(
    release.assets.flatMap((value) => {
      const asset = asRecord(value);
      return typeof asset?.name === "string" ? [[asset.name, asset]] : [];
    }),
  );
  const completeRelease = requiredAssetNames.every((assetName) => {
    const asset = approvedAssets.get(assetName);
    const expectedUrl = `${releaseRoot}/${release.tag_name}/${assetName}`;
    return (
      asset?.state === "uploaded" && asset.browser_download_url === expectedUrl
    );
  });
  const dmgName = `TouchGrassBar_${version}_aarch64.dmg`;
  const dmgUrl = `${releaseRoot}/${release.tag_name}/${dmgName}`;

  return completeRelease ? { url: dmgUrl, version } : null;
}

export async function resolveApprovedDownload(
  fetchLatestRelease: FetchLatestRelease,
): Promise<ApprovedDownload | null> {
  try {
    const response = await fetchLatestRelease(latestReleaseApiUrl, {
      credentials: "omit",
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!response.ok) return null;
    return approvedDownloadFromRelease(await response.json());
  } catch {
    return null;
  }
}

export async function installDownloadResolver(
  documentObject: Document,
  fetchLatestRelease: FetchLatestRelease,
) {
  const links = documentObject.querySelectorAll<HTMLAnchorElement>(
    "[data-download-link]",
  );
  const approvedDownload = await resolveApprovedDownload(fetchLatestRelease);

  if (!approvedDownload) return null;

  for (const link of links) {
    link.href = approvedDownload.url;
    link.dataset.downloadVersion = approvedDownload.version;
  }
  return approvedDownload;
}
