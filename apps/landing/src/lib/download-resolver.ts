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

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : null;
}

export function approvedDownloadFromRelease(
  input: unknown,
): ApprovedDownload | null {
  const release = record(input);
  if (
    !release ||
    release.draft !== false ||
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
  const assetName = `TouchGrassBar_${version}_aarch64.dmg`;
  const expectedUrl = `${releaseRoot}/${release.tag_name}/${assetName}`;
  const approvedAsset = release.assets.find((value) => {
    const asset = record(value);
    return (
      asset?.name === assetName &&
      asset.state === "uploaded" &&
      asset.browser_download_url === expectedUrl
    );
  });

  return approvedAsset ? { url: expectedUrl, version } : null;
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
  const statuses = documentObject.querySelectorAll<HTMLElement>(
    "[data-download-status]",
  );
  const approvedDownload = await resolveApprovedDownload(fetchLatestRelease);

  if (!approvedDownload) {
    for (const status of statuses) {
      status.textContent =
        "The exact download is not available. The GitHub Release page will open.";
    }
    return null;
  }

  for (const link of links) {
    link.href = approvedDownload.url;
    link.dataset.downloadVersion = approvedDownload.version;
  }
  for (const status of statuses) {
    status.textContent = `TouchGrassBar ${approvedDownload.version} for Apple silicon.`;
  }
  return approvedDownload;
}
