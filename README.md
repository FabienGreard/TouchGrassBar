# TouchGrassBar

A macOS menu-bar app for checking Codex and Claude limits, seeing locally observed token activity, and comparing public Token Scores on the Doomerboard.

TouchGrassBar is an open, playful consumer project. It is not an employee-monitoring or productivity product.

## Workspace

- `apps/desktop` — Tauri, React, Vite, and Tailwind CSS
- `apps/landing` — Astro and Tailwind CSS
- `packages/backend` — Convex schema and functions
- `packages/contracts` — shared sanitized data contracts
- `packages/ui` — shared UI primitives and theme
- `packages/tooling` — strict TypeScript and Oxlint configuration

## Commands

Install [Bun](https://bun.sh), the stable Rust toolchain, and the macOS prerequisites for Tauri, then run:

```sh
bun setup
bun env:check
bun quality
bun dev
```

On first use, `setup` creates an anonymous local Convex deployment with no
Convex account. If `.env.local` already selects a deployment, `setup` validates
and preserves that selection. All development commands read the root
`.env.local`, including commands started inside a workspace package.

`bun dev` starts the selected Convex backend, the signed native desktop app,
and the landing site. The desktop runner derives a visible development label
and isolated runtime namespace from the worktree. It uses the stable
`app.touchgrass.bar.dev` bundle identifier and the configured development
provisioning profile for Keychain access.

Use a package command when you need one surface:

```sh
cd apps/desktop && bun dev
cd packages/backend && bun dev
cd apps/landing && bun dev
```

The desktop command starts its required backend. Backend startup never changes
the deployment selection. Select cloud development explicitly when needed:

```sh
bun convex:login
bun run --cwd packages/backend convex deployment select dev
bun env:check
```

The Convex CLI writes the three standard values to the root `.env.local`. You
can also set those values manually from `.env.example`. No package-specific
environment file is required.

`convex:dev` runs the Convex development command for the selected deployment.
`convex:prod` deploys the backend to production and requires explicit production
authority. The backend package also exposes `convex` as a direct Convex CLI
passthrough, like the desktop package exposes `tauri`.

Build a real signed development application bundle with no Vite or Convex
server:

```sh
bun desktop:bundle
```

The hot development app and development bundle use the same development
Keychain service. The release workflow uses the production bundle identifier,
Developer ID signing, hardened runtime, timestamping, notarization, and
stapling. Its release environment must provide the production provisioning
profile through `APPLE_PROVISIONING_PROFILE_BASE64`. The workflow validates the
profile, certificate, final entitlements, embedded profile, notarization ticket,
and Gatekeeper result. `desktop:release` is available only to release CI and
never reads a developer `.env.local`.

Cleanup commands have separate data boundaries:

```sh
bun clean
bun reset
bun reset:bundle
bun reset:release
```

`clean` removes only build output and caches. `reset` removes all development
state in the worktree, then creates a fresh local setup. It preserves every
remote Convex deployment and all production state. The command starts the reset
without a confirmation prompt. `reset:bundle` removes only the packaged
development app data and shared development Keychain items. It preserves the
built app and local Convex data. `reset:release` removes only the release app
data and release Keychain items on this Mac. It requires strong confirmation
and never deletes the production Convex database.

All JavaScript and TypeScript dependency management and scripts run through Bun. TypeScript tests use Vitest; native tests use Cargo. Rust is limited to the Tauri native core.

Read [CONTEXT.md](./CONTEXT.md), [the product definition](./docs/product.md), and [the architecture](./docs/architecture.md) before changing domain behavior.

## License

TouchGrassBar is available under the [MIT License](./LICENSE). Copyright (c) 2026 Fabien Greard.
