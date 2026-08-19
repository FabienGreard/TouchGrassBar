---
name: release
description: Start a governed TouchGrassBar patch, minor, or major SemVer release. Use when the user asks to release, cut a version, or start a new app release.
---

# Release

Use `scripts/release.ts` as the single source of truth for release checks and actions.

1. Get the release level from the user: `patch`, `minor`, or `major`. Ask for it when it is absent. Completion: one level is explicit.
2. Run `bun run release LEVEL`. Report the current tag, next tag, commit, and CI run. Completion: every automated preflight passes and the preview names one next tag.
3. Run `bun run release LEVEL --execute`. Completion: the script reports that the immutable tag was pushed.
4. Find the tag run with `gh run list --workflow release.yml --branch TAG --limit 1`. Report its URL and status, including the `macos-release` approval request. Completion: the user has the exact tag and workflow state.
