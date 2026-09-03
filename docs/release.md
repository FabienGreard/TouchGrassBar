# Release governance and draft candidates

This runbook owns issue #33 release controls. It creates a signed, notarized,
and stapled Apple-silicon draft candidate. It does not publish a Release.
Issue #39 owns publication and `PUBLIC_GO`.

## Proof Budget

Release evidence can contain these public facts:

- configuration name, scope, present or absent state, and update time;
- tag, commit, workflow run, job, and Action version;
- database fixture tag, repository-relative path, SHA-256 value, and test result;
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

Add these names only to the `macos-release` environment. Do not create a
repository or organization copy.

| Kind      | Names                                                                                                                                                                         |
| --------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Secrets   | `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_NOTARY_KEY`, `APPLE_PROVISIONING_PROFILE_BASE64`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` |
| Variables | `APPLE_API_KEY_ID`, `APPLE_API_ISSUER`, `APPLE_SIGNING_IDENTITY`, `TOUCHGRASS_AUTH_SITE_URL`, `TOUCHGRASS_CONVEX_URL`                                                         |

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
configuration. It does not request or emit a protected value.

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

The source fixture manifest must contain one candidate, and its tag must be the
exact new tag. Every published, non-draft stable GitHub Release must have one
official fixture. The official fixture set must match that Release set exactly.
Preview and execution stop if an old official fixture is absent, if another
candidate exists, or if a fixture file or SHA-256 value is invalid.

The fixture generator can create or replace only the current candidate. It
cannot delete or write an official fixture database. Use its check mode before
you prepare a tag:

```sh
bun scripts/generate-database-fixtures.ts --check
```

The check validates every database and manifest hash. It also compares each
official `sourceCommit` with its local Git tag when that tag is available. CI
runs the same check. The complete fixture lifecycle and the schema change
checklist are linked from
[`apps/desktop/src-tauri/tests/fixtures/releases/README.md`](../apps/desktop/src-tauri/tests/fixtures/releases/README.md).

## User-facing release notes

The release preview requires at least one user-facing change. For the best
wording, add one or more `Release-note:` trailers to the commit message:

```text
fix(codex): hide reserve quota lane

Release-note: The Codex usage panel no longer shows the internal reserve limit.
```

Use `Release-note: none` when a `feat`, `fix`, or `perf` commit has no
user-facing effect. Without a trailer, the release-note generator uses the
Conventional Commit subject as a fallback. The release-note generator excludes
release, development, test, documentation, build, and CI scopes.

When one squash commit contains more than one user-facing change, add one
`Release-note:` trailer for each change. An inferred fallback uses only the
top-level Conventional Commit subject. The release-note generator does not infer
release notes from nested commit text in the squash commit body. The release-note
generator reads release-note fields only from the final top-level trailer block.
If the earlier body contains a nested Conventional Commit subject, the
release-note generator ignores the final trailer block. The final trailer block
has an ambiguous owner.

If an uneditable squash commit has incomplete or technical fallback text, add
one later release commit. Put `Release-note-mode: replace` and all reviewed
`Release-note:` trailers in that commit. Only one replacement summary is
allowed in a tag range. The replacement summary replaces every fallback and
trailer from earlier commits in that range. Later commits still contribute their
explicit trailers or fallback subjects. The replacement commit must have at
least one explicit user-facing `Release-note:` trailer.

The generator normally starts at the previous tag. A metadata-only rewrite can
remove that tag from the `main` history. In that case, the generator requires
exactly one first-parent commit. That commit must have the same tree and subject
as the tagged commit. The generator starts at that equivalent commit. Starting
at the equivalent commit prevents duplicate old notes and the loss of a later
repeated patch.

The full-changelog link uses the same equivalent commit as its comparison
baseline. The immutable release tag stays unchanged.

The GitHub Release description puts the user-facing changes and DMG download
first. It keeps signing, notarization, Gatekeeper, updater-signature, asset-size,
and SHA-256 evidence in a collapsed technical section. GitHub shows draft
status, so the description must not contain temporary draft wording. The same
change summary is stored in `latest.json` for the in-app updater.

After the automated preview passes, use the execution command printed by the
script. The script creates and pushes the annotated tag. The tag starts the
release workflow.

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
2. It runs every released database fixture against the candidate code. It
   records a sanitized result that is bound to the tag, commit, and workflow.
3. It installs the certificate, notary key, and provisioning profile only for
   their consuming steps. Private files use runner-temporary storage and mode
   `0600`.
4. It sets the app version from the validated tag. Tauri creates the signed,
   hardened, timestamped, notarized, and stapled app plus its updater archive
   and detached signature.
5. It independently submits the versioned DMG to Apple, requires `Accepted`,
   staples it, and checks Gatekeeper.
6. It checks the app in the updater archive and DMG against the trusted app.
   It also checks arm64-only architecture, version, Developer ID identity,
   public certificate fingerprint, app and DMG signatures, notarization,
   stapling, Gatekeeper, and the Tauri updater signature.
7. It creates `latest.json`, `SHA256SUMS`, and a sanitized trust receipt.
8. It creates a draft GitHub Release and removes temporary release material.

The draft contains these assets:

- `TouchGrassBar_VERSION_aarch64.dmg`;
- `TouchGrassBar_VERSION_aarch64.app.tar.gz` and its `.sig` file;
- `latest.json`;
- `SHA256SUMS`;
- `database-compatibility-VERSION.json`;
- `release-trust-VERSION.json`.

The workflow has no publication step. After all candidate, physical QA,
performance, backend, and evidence gates pass, issue #39 must use the separate
`public-release` approval for the exact draft.

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
