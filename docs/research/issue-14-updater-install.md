# Issue 14: updater and install policy

**Status:** Active research

**Date:** 2026-08-03

**Related issues:** [#32](https://github.com/FabienGreard/TouchGrassBar/issues/32),
[#33](https://github.com/FabienGreard/TouchGrassBar/issues/33), and
[#34](https://github.com/FabienGreard/TouchGrassBar/issues/34). Issue #14 holds
the approved source decision.

**Cleanup condition:** Delete this note after issues #32, #33, and #34 close
and their current updater, release, recovery, and install rules exist in
durable product, release, and QA documentation.

**Promotion target:** Move product behavior into `docs/product.md`, release
controls into `.github/workflows/release.yml` and its release runbook, and
failure cases into the updater and release QA tests owned by issues #32 and
#33.

**Scope:** Updater and install policy that follows the distribution trust
decision in issue #4. This note changes no production code.

## Recommended decision

Ship one public **stable** update channel backed by the latest full GitHub Release. Use a versioned, signed, notarized, and stapled DMG for the first install; use Tauri's signed `.app.tar.gz` artifact for in-app updates. Draft releases remain the human QA gate. Do not expose a beta-channel setting in the MVP.

Check quietly when the first menu-bar panel opens, at most once per 24 hours, and offer a manual **Check for Updates** action in Settings. An available normal update should appear as a persistent, non-modal row. The only primary action should be **Install & Relaunch**: download, verify, install, and immediately restart as one user-initiated operation. A normal update never blocks local quota/history features.

Use roll-forward recovery only. If an update fails, keep the current process usable, show **Retry** and **Download the latest DMG**, and publish a higher SemVer fix. Do not promise automatic rollback or use a downgrade as the normal recovery path.

## Verified capabilities and constraints

### Feed, versions, and channels

- Tauri v2 supports static JSON and dynamic update endpoints. A static manifest supplies a release `version` and per-platform download `url` and `signature`; a dynamic endpoint returns `204` for no update or a JSON release for an update. Endpoint templates can include the current version, target, and architecture. Tauri tries the next configured endpoint only when the preceding endpoint returns a non-success status. ([Tauri updater documentation](https://v2.tauri.app/plugin/updater/))
- The default comparison offers an update only when the remote SemVer is greater than the installed version. A custom comparator can alter that behavior, including permitting downgrades, but this must be deliberately implemented. ([Tauri updater documentation](https://v2.tauri.app/plugin/updater/), [pinned updater 2.10.1 source](https://github.com/tauri-apps/plugins-workspace/blob/updater-v2.10.1/plugins/updater/src/updater.rs))
- Runtime-configured endpoints can model stable and beta channels, but the application must select the endpoint. There is no channel UX supplied by Tauri. ([Tauri updater documentation](https://v2.tauri.app/plugin/updater/))
- GitHub's “latest release” is the most recent published release that is neither a draft nor a prerelease. Drafts and prereleases cannot be marked latest. A stable `latest.json` asset therefore naturally excludes QA drafts and prereleases. ([GitHub Releases REST documentation](https://docs.github.com/en/rest/releases/releases#get-the-latest-release))

**Implication for #14:** use one endpoint such as the `latest.json` asset on `/releases/latest`. A later beta program should use a separate manifest/endpoint and an explicitly opted-in build, not overload the stable feed.

### Manifest and signature requirements

- On macOS, Tauri's updater artifacts are the app bundle archive (`.app.tar.gz`) and its detached signature (`.app.tar.gz.sig`). `bundle.createUpdaterArtifacts` must be enabled, the application must contain the updater public key, and production endpoints must use HTTPS. ([Tauri updater documentation](https://v2.tauri.app/plugin/updater/))
- The manifest's `signature` value is the contents of the `.sig` file, not a path or URL. The updater downloads the archive and verifies the complete byte buffer against that signature and the embedded public key before installation. ([Tauri updater documentation](https://v2.tauri.app/plugin/updater/), [pinned updater 2.10.1 source](https://github.com/tauri-apps/plugins-workspace/blob/updater-v2.10.1/plugins/updater/src/updater.rs))
- The detached Tauri signature authenticates the update archive; it is not a separate signature over `latest.json`. HTTPS protects manifest transport. Extra JSON fields are available to Rust through `raw_json`, so product policy metadata can be added, but the updater does not enforce those fields itself. ([pinned updater 2.10.1 source](https://github.com/tauri-apps/plugins-workspace/blob/updater-v2.10.1/plugins/updater/src/updater.rs))

**Implication for #14:** keep update checks and policy interpretation in Rust, validate any custom metadata, and expose only sanitized update status to React. The signature check remains the non-bypassable installation boundary.

### Prompting, progress, and relaunch

- Registering the updater plugin does not schedule checks or display prompts. The application explicitly calls `check`, then download/install APIs, and can receive download progress callbacks. The official example explicitly restarts after a successful install. ([Tauri updater documentation](https://v2.tauri.app/plugin/updater/))
- Tauri's process restart requests application exit and starts the executable again; the pinned Rust implementation provides an exit-request path intended to deliver lifecycle events before restart. ([Tauri Process plugin](https://v2.tauri.app/plugin/process/), [Tauri 2.11.5 restart source](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/app.rs#L577-L624))

**Implication for #14:** before installation, flush SQLite writes and pause refresh/sync work. Call restart only after installation succeeds. Do not install an update and leave the old process running behind a “Later” button.

### Failure recovery and rollback

- In updater 2.10.1, network and signature failures occur before installation. On macOS, the archive is extracted to a temporary directory before the current app is moved. ([pinned updater 2.10.1 source](https://github.com/tauri-apps/plugins-workspace/blob/updater-v2.10.1/plugins/updater/src/updater.rs))
- The pinned macOS installer then renames the current app into a temporary backup and renames the extracted replacement into the app path. If permissions require it, it runs an administrator-authorized AppleScript replacement. The ordinary rename branch has no explicit restoration step if moving the replacement fails after the current app was moved. This is an inference from the exact pinned source, not a documented transactional rollback guarantee. ([pinned updater 2.10.1 source](https://github.com/tauri-apps/plugins-workspace/blob/updater-v2.10.1/plugins/updater/src/updater.rs))

**Implication for #14:** network, download, and signature errors can safely return to the running app; a mid-replacement filesystem failure needs physical fault testing and a DMG recovery path. The product must not claim atomic rollback until that test proves it.

### Signing, notarization, and first install

- Apple requires Developer ID signing for software distributed outside the Mac App Store, and notarization lets Gatekeeper verify that distributed software passed Apple's checks. A notarization ticket can be stapled for offline validation. ([Apple Developer ID](https://developer.apple.com/support/developer-id/), [Apple notarization documentation](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution))
- GitHub supports a stable URL for the latest release and a direct latest-asset URL only when the asset filename is stable. ([GitHub linking-to-releases documentation](https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases#linking-to-the-latest-release))

**Implication for #14:** the landing page should resolve the latest full release and link to its exact versioned DMG asset, with `/releases/latest` as the fallback. The first-install DMG needs both the inner app and final outer DMG signed/notarized/stapled as decided in #4. In-app update archives must contain that same signed/notarized/stapled app, plus the Tauri signature; they do not reinstall through the DMG.

## Product policy for issue #14

| Topic            | Recommended policy                                                                                                                                                                                                                |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Public channel   | Stable only; latest full GitHub Release. Drafts are QA staging. No beta selector in MVP.                                                                                                                                          |
| Check cadence    | On first panel open, at most once per 24 hours; manual Settings action always available. No polling on every provider refresh.                                                                                                    |
| Normal prompt    | Persistent non-modal row with version and short notes; **Install & Relaunch** plus **Later**. Re-offer after 24 hours, not on every panel open.                                                                                   |
| Install behavior | After explicit consent: show progress, verify signature, install, immediately restart. Never silently auto-install.                                                                                                               |
| Failure          | Preserve current UI where possible; offer **Retry** and **Download latest DMG**. Do not loop prompts.                                                                                                                             |
| Recovery         | Publish a higher SemVer fix. Keep prior releases for audit, but do not advertise downgrade as user recovery.                                                                                                                      |
| Mandatory update | No blanket forced update for local features. A server-owned minimum version may disable only incompatible online/public features and return a typed `upgrade_required` state. Settings and the update path must remain reachable. |
| Critical update  | Persistent warning and emphasized install action. Hard-block online/public features only when protocol or security compatibility requires it.                                                                                     |
| First install    | Landing-page versioned DMG; drag to Applications; normal updater cadence begins after launch.                                                                                                                                     |

### Minimum-version metadata

The stock Tauri manifest has no documented `mandatory` or `minimum_supported_version` behavior. If needed, add strictly validated custom fields to the update response, for example `severity` and `minimum_supported_version`, and interpret them in Rust via `raw_json`. Old clients cannot honor fields they do not understand, so the backend must enforce any real minimum for its own network protocol. The archive still cannot install unless its Tauri signature validates.

## Current repository gap

The desktop currently pins Tauri `2.11.5`, updater `2.10.1`, and process `2.3.1`. [`tauri.conf.json`](../../apps/desktop/src-tauri/tauri.conf.json) still has version `0.0.0`, a placeholder updater key, no checked-in endpoint, and no `createUpdaterArtifacts` setting. [`lib.rs`](../../apps/desktop/src-tauri/src/lib.rs) registers the updater and process plugins but has no check/download/install/restart coordinator. The [release workflow](../../.github/workflows/release.yml) injects an endpoint and key and creates a draft release, but issue #4 already records the remaining signing/notarization and release-hardening work.

## Evidence required before approval becomes implementation

1. Latest stable feed ignores a newer draft/prerelease and offers a higher stable SemVer.
2. Physical previous-version-to-new-version update on Apple silicon, with immediate relaunch and persisted SQLite/Keychain state.
3. Offline, 404, interrupted download, wrong signature, truncated archive, permission denial/cancel, low disk, and termination during replacement.
4. Recovery from a non-launching version via the latest versioned DMG without deleting local data.
5. Gatekeeper checks for the fresh DMG and the updated app, both online and offline after stapling.
6. Mandatory/minimum-version behavior leaves local quota/history, Settings, and the recovery download reachable while refusing only incompatible online/public operations.

Until these tests pass, “signed update support” means updater plumbing exists; it does not mean rollback, mandatory-update UX, or release recovery is proven.

## Primary sources

- [Tauri v2 updater plugin](https://v2.tauri.app/plugin/updater/)
- [Tauri updater 2.10.1 source](https://github.com/tauri-apps/plugins-workspace/blob/updater-v2.10.1/plugins/updater/src/updater.rs)
- [Tauri v2 process plugin](https://v2.tauri.app/plugin/process/)
- [Tauri 2.11.5 restart source](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/src/app.rs#L577-L624)
- [GitHub Releases REST API](https://docs.github.com/en/rest/releases/releases)
- [GitHub release links](https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases)
- [Apple Developer ID](https://developer.apple.com/support/developer-id/)
- [Apple notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
