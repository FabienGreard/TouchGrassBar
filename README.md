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

`bun desktop` derives a visible development-instance label and color from the
current worktree and branch. It also selects an isolated localhost port and
native application identifier so parallel worktrees can run together. Use
`bun desktop:preview` for the browser-only preview. Override the visible values
when needed:

```sh
bun desktop --label "Cache refresh" --accent violet
bun desktop:preview --label "Cache refresh" --accent violet
```

All JavaScript and TypeScript dependency management and scripts run through Bun. TypeScript tests use Vitest; native tests use Cargo. Rust is limited to the Tauri native core.

Read [CONTEXT.md](./CONTEXT.md), [the product definition](./docs/product.md), and [the architecture](./docs/architecture.md) before changing domain behavior.

## License

TouchGrassBar is available under the [MIT License](./LICENSE). Copyright (c) 2026 Fabien Greard.
