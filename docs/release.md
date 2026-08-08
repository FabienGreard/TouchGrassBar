# Release governance and draft candidates

This runbook owns issue #33 release controls. It creates a signed, notarized,
and stapled Apple-silicon draft candidate. It does not publish a Release.
Issue #39 owns publication and `PUBLIC_GO`.

## Proof Budget

Release evidence can contain these public facts:

- configuration name, scope, present or absent state, and update time;
- tag, commit, workflow run, job, and Action version;
- toolchain version;
- artifact name, byte size, and SHA-256 value;
- Developer ID identity and public certificate SHA-256 value;
- signing, hardened runtime, timestamp, notarization, stapling, Gatekeeper,
  and updater-signature result.

Evidence must not contain a credential value, encoded credential, credential
length, private-key fingerprint, raw Apple response, raw log, or runner path.
Do not enable shell tracing in a release job.

## Governed GitHub state

The canonical policy is
`.github/release-governance.json`. Activate it only after the commit that
contains the release workflows is on remote `main`:

```sh
bun run release:governance --apply
```

The command refuses a dirty worktree, a non-`main` branch, or a commit that is
not exact remote `main`. It configures these controls:

- `macos-release` and `public-release` have `FabienGreard` as the sole
  reviewer. Self-review is permitted. The wait timer is zero. Only `v*` tags
  can enter either environment.
- Stable `v*` tags cannot be updated or deleted. No actor can bypass this tag
  rule.
- GitHub Actions uses read-only default token access and selected Actions only.
  Readable stable Action versions are permitted.
- GitHub immutable Releases are enabled.

GitHub does not expose the administrator-bypass switch through its public
environment API. In repository **Settings > Environments**, open each release
environment and clear **Allow administrators to bypass configured protection
rules**. This manual check is mandatory.

Add these names only to the `macos-release` environment. Do not create a
repository or organization copy.

| Kind | Names |
| --- | --- |
| Secrets | `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_NOTARY_KEY`, `APPLE_PROVISIONING_PROFILE_BASE64`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` |
| Variables | `APPLE_API_KEY_ID`, `APPLE_API_ISSUER`, `APPLE_SIGNING_IDENTITY`, `TOUCHGRASS_AUTH_SITE_URL`, `TOUCHGRASS_CONVEX_URL` |

The production provisioning profile is release signing material. It stays in
the secret-bearing environment because the Profile Keychain contract needs its
application identifier and access group. The two TouchGrass service URLs are
public build configuration, so they are environment variables, not secrets.

`public-release` has no secret and no variable.

The updater endpoint and public key are public source configuration in
`apps/desktop/src-tauri/tauri.conf.json`. The endpoint is the stable GitHub
Release asset URL. Replace the fail-closed public-key marker with the public
output of the owner-custodied Tauri updater key. Never add the private key to a
file, commit, command argument, CI artifact, issue, or chat.

After the names and public key are ready, run this command before every
candidate tag and approval:

```sh
bun run release:governance --verify
```

This command requests name and update-time fields only. It checks both
environment scopes, repository-level duplicates, reviewer and tag controls,
Action policy, immutable tags, immutable Releases, and the public updater
configuration. It does not request or emit a protected value. Its PASS state
covers automated checks only. Confirm the two administrator-bypass switches in
the GitHub interface before you push the tag.

## Candidate trigger and approval

Use an annotated tag with exact `vMAJOR.MINOR.PATCH` form. Leading zeroes,
prerelease text, and build text are invalid. The tag commit must be a member of
`main` and must have a successful exact-head `main` CI run.

Preview the next governed version with one of these commands:

```sh
bun run release patch
bun run release minor
bun run release major
```

After the automated preview passes and the mandatory administrator-bypass
check is complete, use the execution command printed by the script. The script
creates and pushes the annotated tag. The tag starts the release workflow.

The unprivileged `validate` job proves these conditions before GitHub creates a
`macos-release` approval request. A rejected or failed candidate consumes its
tag and version. Do not move or delete the tag. Fix the problem and use a
higher SemVer.

Approval is for one workflow run, tag, and commit. Cancel a pending run if its
commit, tag, workflow, lockfile, Action version, credential, environment policy, or
public release configuration changes. Start a new run with a higher tag and
give a fresh approval.

## Protected build

The protected job uses an arm64 macOS 15 GitHub-hosted runner. It does this
work:

1. It records required configuration as booleans. A missing name or the
   updater public-key marker stops the job.
2. It installs the certificate, notary key, and provisioning profile only for
   their consuming steps. Private files use runner-temporary storage and mode
   `0600`.
3. It sets the app version from the validated tag. Tauri creates the signed,
   hardened, timestamped, notarized, and stapled app plus its updater archive
   and detached signature.
4. It independently submits the versioned DMG to Apple, requires `Accepted`,
   staples it, and checks Gatekeeper.
5. It checks the app in the updater archive and DMG against the trusted app.
   It also checks arm64-only architecture, version, Developer ID identity,
   public certificate fingerprint, app and DMG signatures, notarization,
   stapling, Gatekeeper, and the Tauri updater signature.
6. It creates `latest.json`, `SHA256SUMS`, and a sanitized trust receipt.
7. It creates a draft GitHub Release and removes temporary release material.

The draft contains these assets:

- `TouchGrassBar_VERSION_aarch64.dmg`;
- `TouchGrassBar_VERSION_aarch64.app.tar.gz` and its `.sig` file;
- `latest.json`;
- `SHA256SUMS`;
- `release-trust-VERSION.json`.

The workflow has no publication step. After all candidate, physical QA,
performance, backend, and evidence gates pass, issue #39 must use the separate
`public-release` approval for the exact draft.

Run the [macOS release gates](macos-release-gates.md) against the exact app and
DMG from this draft. Keep its sanitized PASS receipt and required physical
checklist with the candidate evidence. A rebuilt or changed bundle requires a
new candidate and a new gate run.

## Retention and key response

Keep a rejected or abandoned draft for 90 days, then delete the draft and its
assets. Keep its immutable tag. Keep published installers, updater files,
checksums, evidence, and attestations without a time limit.

The updater key has no calendar rotation. Keep three protected copies: the
environment secret, the owner-only recovery copy, and one encrypted offline
copy in another failure domain. Do one presence-only recovery drill each year.
Use a predecessor-signed bridge Release for planned key rotation. Keep that
bridge latest for 90 days.

Rotate the App Store Connect key every 12 months and after a custody or
security event. Start Developer ID certificate replacement 60 days before
expiry. A suspected private-key or certificate exposure freezes this workflow
and the stable feed. Do not create a bridge with an exposed updater key. Ship a
new signed and notarized DMG and require manual reinstall.

Primary tool contracts:

- [GitHub deployment environments](https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments)
- [GitHub immutable Releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)
- [Tauri updater](https://v2.tauri.app/plugin/updater/)
- [Tauri macOS signing](https://v2.tauri.app/distribute/sign/macos/)
