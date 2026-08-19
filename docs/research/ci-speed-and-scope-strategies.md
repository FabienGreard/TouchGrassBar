# CI speed and scope strategies

**Status:** Active research

**Date:** 2026-08-05

**Related issues:** [#33](https://github.com/FabienGreard/TouchGrassBar/issues/33),
[#35](https://github.com/FabienGreard/TouchGrassBar/issues/35), and
[#37](https://github.com/FabienGreard/TouchGrassBar/issues/37)

**Cleanup condition:** Delete this note after issues #33, #35, and #37 close
and the current CI and release proof rules exist in durable workflows and
release documentation.

**Promotion target:** Keep executable controls in `.github/workflows/ci.yml`
and `.github/workflows/release.yml`. Move operator decisions and manual gates
into the release runbook delivered by issues #33 and #37.

**Scope:** GitHub Actions dependency setup, caching, and pull request, `main`, and release proof

## Recommended decision

Keep two independent CI jobs:

1. **Code quality** on Ubuntu.
2. **Native app** on macOS 15.

Do not add a shared dependency-install job. Add a Cargo dependency cache first. Do not cache `node_modules`. Do not add a Bun package cache unless install time becomes material.

Use different proof for each event:

| Event          | Code quality                                              | Native proof                                           | Bundle and release proof                                               |
| -------------- | --------------------------------------------------------- | ------------------------------------------------------ | ---------------------------------------------------------------------- |
| Pull request   | Full repository quality on Ubuntu                         | `cargo fmt`, `cargo test`, and `cargo clippy` on macOS | None                                                                   |
| Push to `main` | Full repository quality on Ubuntu                         | Full native checks on macOS                            | Unsigned app bundle                                                    |
| Version tag    | Verify that the exact tag commit has successful `main` CI | Do not repeat broad tests                              | Build, sign, notarize, publish a draft, then do a manual visual review |

This direction contains no Playwright, screenshot, PNG, or browser-preview job. Unit and exact layout-invariant tests remain in Code quality. A person checks the built macOS app before a draft release becomes public.

## Current timing evidence

The install step is small compared with the current work:

| Hosted run                                                                                | Bun install | Code quality | Rust format, test, and lint | Unsigned bundle |      Whole job |
| ----------------------------------------------------------------------------------------- | ----------: | -----------: | --------------------------: | --------------: | -------------: |
| [Run 30866644712](https://github.com/FabienGreard/TouchGrassBar/actions/runs/30866644712) |         6 s |         94 s |                        47 s |           147 s |          311 s |
| [Run 30867212021](https://github.com/FabienGreard/TouchGrassBar/actions/runs/30867212021) |         7 s |        110 s |                        60 s |           177 s |          373 s |
| [Run 30957867979](https://github.com/FabienGreard/TouchGrassBar/actions/runs/30957867979) |         9 s |        167 s |                 Not reached |     Not reached | Failed earlier |

The local `node_modules` tree is about 948 MiB. Passing that tree between jobs would exchange several seconds of installation for a large upload, download, and extraction path.

## Can one job install packages for the other jobs?

Not through a shared filesystem. GitHub gives each hosted job a fresh virtual machine. Only steps in the same job share files directly. An earlier job can upload an artifact, but GitHub defines artifacts as produced files passed between jobs and caches as the dependency-reuse tool. ([GitHub-hosted runners](https://docs.github.com/en/actions/how-tos/manage-runners/github-hosted-runners/use-github-hosted-runners), [dependency caching](https://docs.github.com/en/actions/concepts/workflows-and-actions/dependency-caching))

A shared install job would have these costs:

- It would block later jobs until checkout, install, upload, and download finish.
- It would reduce parallel feedback.
- A Linux dependency tree cannot be treated as the macOS dependency tree because packages can contain platform-specific files.
- Every consumer would still need a lockfile-safe dependency validation step.

Use `bun ci` in each job that needs JavaScript dependencies. Bun documents it as equivalent to `bun install --frozen-lockfile`. ([Bun install](https://bun.sh/docs/pm/cli/install))

A local composite action could remove repeated YAML, but it would not remove repeated runtime work. The current setup is too short to justify that extra abstraction.

## Cache choices

### Keep: Bun executable cache

`oven-sh/setup-bun@v2` already caches the downloaded Bun executable unless `no-cache` is set. Its `cache-hit` output refers to that executable, not project dependencies. ([setup-bun](https://github.com/oven-sh/setup-bun))

### Do not add now: Bun package cache

Bun stores downloaded packages in `~/.bun/install/cache`. This directory can use `actions/cache`, keyed by operating system, architecture, Bun version, and `bun.lock`. Every job must still run `bun ci`. ([Bun global cache](https://bun.sh/docs/pm/global-cache), [GitHub dependency caching](https://docs.github.com/en/actions/concepts/workflows-and-actions/dependency-caching))

The measured 4–9 second install does not justify cache restore and save traffic. A Bun cache is useful only if registry downloads become a measured source of delay or failure.

### Do not use: `node_modules` cache or artifact

This is the largest and most platform-sensitive choice. It adds stale-tree and lifecycle-script risk, and transfer time can exceed a clean Bun install. Cache downloaded packages instead if this area later needs work.

### Add first: Cargo dependency cache

The native path is the main cost. `Swatinem/rust-cache` caches Cargo registry data and compiled dependency artifacts. Its keys include the Rust toolchain, Cargo files, configuration, and compiler environment. It removes workspace and incremental artifacts before save and includes a macOS cache-corruption workaround. This repository has a committed `Cargo.lock`, which makes the cache more effective. ([rust-cache source and documentation](https://github.com/Swatinem/rust-cache#readme))

Configure the workspace as `apps/desktop/src-tauri -> target`. Let pull requests restore the trusted base-branch cache, but save new cache entries only on `main`. Pin the action to a full commit SHA. GitHub recommends full-length commit pinning for third-party actions. ([GitHub secure-use guidance](https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions#using-third-party-actions))

### Optional later: Turborepo cache and affected tasks

The current `.turbo` cache is about 16 MiB. GitHub cache storage can share it across runs, and Turborepo also supports an external Remote Cache. Turborepo can run only affected package tasks in CI. It falls back to running all tasks when Git history is insufficient, which is safe but slower. ([Turborepo GitHub Actions](https://turborepo.com/docs/guides/ci-vendors/github-actions), [constructing CI](https://turborepo.com/docs/crafting-your-repository/constructing-ci))

Do not depend on this cache until the task model is audited. The current `build.outputs` value is empty, and environment inputs are not declared. Turborepo states that undeclared outputs are not restored and undeclared environment inputs can cause an incorrect cache hit. ([Turborepo caching](https://turborepo.com/docs/crafting-your-repository/caching))

## Strategy options

### A. One macOS job for everything

**Pros**

- One checkout and one Bun install.
- The fewest workflow concepts.
- No cross-job cache or artifact coordination.

**Cons**

- About five to six minutes in the measured successful runs.
- A Code quality failure hides Native app failures.
- Ubuntu cannot provide early, low-cost feedback.
- Every pull request pays for bundling.

Use this only when workflow simplicity is more important than feedback speed.

### B. Two full parallel jobs on every event

**Pros**

- Code quality and Native app failures arrive independently.
- The wall time is the slower job, not their sum.
- It is simple and has no change classifier.

**Cons**

- Two Bun installs when the native job bundles.
- Pull requests, `main`, and releases repeat broad work.
- Every pull request pays the full native bundle cost.

This is a safe baseline. The repeated install is only a small cost.

### C. Event-tiered two-job workflow — recommended

**Pros**

- Pull requests get full correctness checks without waiting for packaging.
- The pull-request Native app job does not need Bun because it does not bundle.
- `main` remains the exact-commit integration and bundle gate.
- A release repeats only release-specific work after it proves the tag points to a green `main` commit.
- Cargo cache writes come only from trusted `main` runs.

**Cons**

- A packaging-only defect can first appear on `main`.
- The release workflow needs an exact-SHA CI check before it can use signing secrets.
- `main` must be protected from direct, unchecked updates.

This gives the best current balance of speed, reliability, and simple ownership.

### D. Affected-only pull requests with Turborepo caching

**Pros**

- Unchanged packages do not repeat lint, typecheck, test, or build work.
- It scales better as the repository grows.
- A small `.turbo` cache can reuse deterministic proof across runs.

**Cons**

- Root contracts and React Doctor still need explicit handling.
- The package graph, task inputs, outputs, and environment inputs must be correct.
- Change classification can miss proof if it is maintained as a manual path list.
- Workflow-level path filters can leave required checks in `Pending`; GitHub recommends against skipping a required workflow this way. ([GitHub required-check guidance](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks))

Use this after the two-job event policy is stable and the Turborepo task contract is explicit.

## Required reliability controls

1. Require the pull-request Code quality and Native app checks before merge.
2. Keep `pull_request` checkout on its default merge commit. GitHub then tests the proposed merged result rather than only the branch head. ([GitHub pull-request event](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#pull_request))
3. If a merge queue is enabled, add the `merge_group` event or required checks will not run for the queue. ([GitHub required-check guidance](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks))
4. Do not tag a release commit until the exact `main` SHA has green Code quality, Native app, and unsigned bundle results.
5. Make the release workflow fail closed when that exact-SHA proof is absent. Do not silently rerun against another ref.
6. Keep the release as a draft until signing, notarization, installation, launch, and the manual visual review pass.

Branch protection could not be read during this research because the GitHub API request failed. The event-tiered strategy is not safe until these controls are confirmed.

## Primary sources

- [GitHub-hosted runners](https://docs.github.com/en/actions/how-tos/manage-runners/github-hosted-runners/use-github-hosted-runners)
- [GitHub dependency caching](https://docs.github.com/en/actions/concepts/workflows-and-actions/dependency-caching)
- [GitHub required status checks](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks)
- [Bun install and `bun ci`](https://bun.sh/docs/pm/cli/install)
- [Bun global cache](https://bun.sh/docs/pm/global-cache)
- [`setup-bun`](https://github.com/oven-sh/setup-bun)
- [`rust-cache`](https://github.com/Swatinem/rust-cache#readme)
- [Turborepo GitHub Actions](https://turborepo.com/docs/guides/ci-vendors/github-actions)
- [Turborepo CI construction](https://turborepo.com/docs/crafting-your-repository/constructing-ci)
- [Turborepo caching](https://turborepo.com/docs/crafting-your-repository/caching)
