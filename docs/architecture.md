# Architecture

## System shape

TouchGrassBar is a Bun-managed Turborepo with one native desktop product, one static landing site, one Convex backend, and shared packages.

### `apps/desktop`

A Tauri macOS menu-bar application. Rust is the trusted native core; React and Vite render sanitized application state with Tailwind CSS.

Rust owns:

- Codex and Claude detection
- Provider-native limit retrieval
- Local usage parsing and historical aggregation
- Model-aware API-equivalent cost estimation
- Keychain credentials and session material
- Local caching and offline reconciliation
- Background refresh
- Identity creation and restoration
- Convex reads and synchronization
- Launch at login and update orchestration

React owns:

- The compact menu-bar panel
- Onboarding and settings presentation
- Loading, stale, unavailable, and error presentation
- User intent delivered to Rust through narrow Tauri commands

React cannot read provider source material, credentials, cookies, raw logs, local paths, or Convex session material. It receives versioned sanitized data-transfer objects.

### `apps/landing`

An Astro and Tailwind CSS static marketing and distribution site. It has no authenticated or live product surface.

### `packages/backend`

Convex owns public Tokenmaxxer identities, Active Mac authority, revisioned Usage Buckets, server-derived daily usage, materialized scores, My Tokenmaxxers, and Doomerboard projections. Better Auth owns generated-credential hashing and sessions.

The backend rejects raw provider material and accepts only validated cumulative daily snapshots from the current Active Mac. Convex calculates all daily totals, combined scores, ranks, and public projections. Global Doomerboards use one namespaced Aggregate component; My Tokenmaxxers uses bounded indexed reads and in-memory sorting. A rate limiter protects synchronization, migrations own repairs, and a daily UTC cron expires rolling windows.

### `packages/contracts`

Shared TypeScript types and validators only for sanitized Rust-to-React Tauri IPC. Convex owns and generates its separate API and data-model types. The Rust-to-TypeScript contract-generation mechanism remains unresolved.

### `packages/ui`

Shared React components, icons, CSS variables, and Tailwind configuration used where sharing does not force desktop-specific behavior into the landing site.

### `packages/tooling`

Shared strict TypeScript and Oxlint configuration.

## Trust boundaries

1. Rust reads local provider sources and converts them into sanitized Quota Snapshots and usage observations.
2. React receives sanitized snapshots through Tauri commands and cannot cross the local-data boundary.
3. Rust reduces observations into UTC Daily Usage Aggregates before synchronization.
4. Convex authenticates the Active Mac, validates aggregate shape and ownership, and updates public Doomerboards.
5. Rust reads public Doomerboard data and gives React only the presentation data it needs.

## Local-first behavior

The trusted native core remains useful when the network or Convex is unavailable. It caches sanitized provider state and pending Daily Usage Aggregates locally, labels freshness explicitly, and resumes synchronization when connectivity returns.

Identity and social features may be unavailable offline, but provider limits and locally observed history must not be blocked by backend readiness.

## Process model

The application runs as a menu-bar-only Tauri process with no Dock icon. The tray interaction controls a compact panel, while onboarding and settings use a separate window. Background refresh and update checks run in the native process.

The Tauri viability spike validated the implementation path and identified the remaining physical and release gates documented in `docs/spikes/tauri-menubar-viability.md`.
