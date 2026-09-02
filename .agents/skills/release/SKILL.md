---
name: release
description: Start and verify a governed TouchGrassBar patch, minor, or major SemVer release with accurate user-facing GitHub Release notes. Use when the user asks to release, cut a version, or start a new app release.
---

# Release

Use `scripts/release.ts` as the single source of truth for release checks and actions.

1. Get the release level from the user: `patch`, `minor`, or `major`. Ask for it when it is absent. Completion: one level is explicit.
2. Run `bun run release LEVEL`. Compare the generated notes with the actual user-facing changes in the tag range. Use the `Release-note:` trailer rules in `docs/release.md`. Stop before tag creation when the notes are incomplete, misleading, or too technical. Report the current tag, next tag, commit, CI run, and release notes. Completion: every automated preflight passes, the preview names one next tag, and the notes describe what users receive.
3. Run `bun run release LEVEL --execute`. Completion: the script reports that the immutable tag was pushed.
4. Find the tag run with `gh run list --workflow release.yml --branch TAG --limit 1`. Report its URL and status, including the `macos-release` approval request. Never approve the protected environment for the user. Completion: the user has the exact tag and workflow state.
5. After the protected workflow succeeds, inspect the release with `gh release view TAG --json url,isDraft,body,assets`. Confirm that its description matches the actual changes, names the correct DMG, includes the collapsed technical verification and full changelog, and contains no stale draft text after publication. If needed, correct only the description; preserve the tag, title, assets, draft or public state, latest flag, and prerelease state. Completion: report the exact release URL and the verified description state.
