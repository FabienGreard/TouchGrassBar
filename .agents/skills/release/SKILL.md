---
name: release
description: Release TouchGrassBar to production when the user requests a patch, minor, or major release. Complete backend deployment, signed builds, publication, and update-feed checks.
---

# Release

Use `scripts/release.ts` as the single source of truth for release checks and actions.

## Authorization

A release request and one explicit level authorize this complete production
workflow. Ask for `patch`, `minor`, or `major` only when the level is absent.
Use the level already given in the conversation.

This authorization includes release preparation, required fixes and fixtures,
commit and push to `main`, the production backend deploy and canaries, tag
creation, workflow dispatch, environment reviews by the authenticated permitted
reviewer, and publication. Carry it through retries and required candidate fixes.
Execute these steps without further confirmation prompts.

Keep validation, signing, protected environments, and immutable tags in effect.
Resolve routine failures within this scope. Request user action only for a
concrete external blocker, such as missing credentials, an unknown target, a
review that the current identity cannot submit, or a change outside release scope.

## Workflow

1. Read the release level from the conversation. Completion: one level is explicit.
2. Prepare a clean `main` candidate. Preserve unrelated work. Follow the fixture lifecycle and `Release-note:` rules in `docs/release.md`. Commit and push the candidate, wait for successful CI for that exact commit, and run `bun run release LEVEL`. Correct incomplete or technical notes before continuing. Completion: all preflight checks pass and the preview reports the current tag, next tag, commit, CI run, and accurate user-facing notes.
3. Invoke `convex-deploy-guard`, identify and announce the production target, and run a dry deployment from the clean candidate checkout. Deploy that exact backend, then verify one successful production synchronization and one populated production read. This applies to every release because the desktop and backend share synchronization validators. Completion: record the deployment, candidate commit, deploy result, synchronization result, and read result before tag creation.
4. Run `bun run release LEVEL --execute`. Completion: the script reports that the immutable tag was pushed.
5. Find the tag run with `gh run list --workflow release.yml --branch TAG --limit 1`. Verify the tag, commit, successful CI, and pending environment before submitting its permitted review through `gh api`. Continue watching the build. Completion: the exact tag workflow succeeds and creates the signed, notarized draft. If a tag fails, keep it immutable and prepare the next valid version under the same release authorization.
6. Inspect the draft with `gh release view TAG --json url,isDraft,body,assets`. Verify the expected DMG, updater archive and signature, checksums, database compatibility evidence, and release trust evidence. Check that the description matches the changes, names the correct DMG, and includes collapsed technical verification and the full changelog. Correct the description when needed. Completion: the draft and its assets pass all release checks.
7. Complete the configured publication workflow, including any permitted `public-release` review. If publication is manual, publish the verified draft with `gh release edit TAG --draft=false --latest`. Preserve its tag, title, assets, and prerelease state. Fetch the public release and stable `latest.json` endpoint again. Completion: the release is public, the update feed names this version and its signed archive, and the final report gives the release URL and verified result.
