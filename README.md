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
bun install
bun quality
bun desktop
```

All JavaScript and TypeScript dependency management and scripts run through Bun. TypeScript tests use Vitest; native tests use Cargo. Rust is limited to the Tauri native core.

Read [CONTEXT.md](./CONTEXT.md), [the product definition](./docs/product.md), and [the architecture](./docs/architecture.md) before changing domain behavior.
