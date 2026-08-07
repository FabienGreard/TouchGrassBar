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
- Profile creation and restoration
- Convex reads and synchronization
- Launch at login and update orchestration

React owns:

- The compact menu-bar panel
- Onboarding and settings presentation
- Loading, stale, unavailable, and error presentation
- User intent delivered to Rust through narrow Tauri commands

React cannot read provider source material, cookies, raw logs, local paths, Convex session material, or local storage. Credentials remain native-owned, except that Profile Settings may receive the Recovery Key through one narrow command after the Tokenmaxxer selects **View**. It receives versioned Sanitized Desktop State and bounded view data for all other presentation. Release WebViews are network-dark; development may allow only the localhost Vite/HMR connection.

#### Desktop React module layout

`App.tsx` is only the production surface router. Application-owned components
are grouped by cohesive domain: `components/panel`,
`components/screens/onboarding`, `components/screens/settings`, and
`components/dialogs`. Onboarding keeps its flow definition, coordinator, and
individual step components together in one flat domain folder; a component
does not receive a one-file folder merely for symmetry.

`native-state` owns the production delivery contract and Tauri adapter. `dev`
owns browser adapters, fixture values, query-string scenario parsing, preview
controls, and preview-only styling. The development entry point is dynamically
loaded by `main.tsx`; production modules never import from `dev`. The dependency
direction is `dev` → desktop product modules → `packages/ui`.

The root development runner starts the selected Convex backend, desktop, and
landing tasks in one owned process group. Package commands delegate to scoped
root commands. The desktop task derives one bounded display identity from the
current branch and worktree. It supplies that identity only through development
environment values and an ignored Tauri configuration overlay. The same Vite
server serves the native WebView and browser preview. Parallel worktrees receive
separate desktop localhost ports, runtime namespaces, and local application data
locations. They share the stable development bundle identifier required by the
development signing profile. Production configuration, product modules, and
Sanitized Desktop State do not contain the development identity.

### `apps/landing`

An Astro and Tailwind CSS static marketing and distribution site. It has no authenticated or live product surface.

### `packages/backend`

Convex owns public Tokenmaxxer Profiles, Active Mac authority, revisioned Usage Buckets, server-derived daily usage, materialized scores, My Tokenmaxxers, and Doomerboard projections. Better Auth owns generated-credential hashing and sessions.

The backend rejects raw provider material and accepts only validated cumulative daily snapshots from the current Active Mac. Convex calculates all daily totals, combined scores, ranks, and public projections. Global Doomerboards use one namespaced Aggregate component; My Tokenmaxxers uses bounded indexed reads and in-memory sorting. A rate limiter protects synchronization, migrations own repairs, and a daily UTC cron expires rolling windows.

#### Development deployment isolation

Each worktree owns its ignored `.convex/` state and root `.env.local`. The
standard `CONVEX_DEPLOYMENT`, `CONVEX_URL`, and `CONVEX_SITE_URL` values select
the backend used by every development command. Setup creates an anonymous local
deployment and private Better Auth secret only when no deployment is selected.
It does not replace a developer's explicit cloud development selection. The
development runner keeps a selected local backend active during native Profile
tests. Startup never rewrites the selected environment.

Default repository setup selects local development. Cloud development and
production require an explicit Convex CLI command and human authorization.
Local success is development evidence only and never qualifies as Backend
Readiness Evidence. This decision is recorded in [ADR 0014](adr/0014-isolate-agent-worktrees-with-local-convex.md).

### `packages/contracts`

Generated TypeScript types and strict runtime validators only for sanitized Rust-to-React Tauri IPC. Sanitized Rust DTOs are canonical; generation is deterministic and CI fails when checked-in bindings drift. Convex owns and generates its separate API and data-model types.

### `packages/ui`

`packages/ui` stays business-stateless. It owns reusable controlled
presentation modules—including the Profile and Coding Provider cards—plus
icons, CSS variables, and Tailwind configuration. Product composition,
workflow state, domain validation, policy, and native intents remain in the
owning application.

### `packages/tooling`

Shared strict TypeScript and Oxlint configuration.

## Trust boundaries

1. Rust reads local provider sources and immediately reduces them into private parser metadata, sanitized Quota Snapshots, and Daily Usage Aggregates.
2. DTOs in the sanitized contract and the deliberate Profile Settings Recovery Key reveal may serialize across Tauri IPC. Privileged provider and other authentication types are separate and non-serializable through commands.
3. React sends narrow typed intents and receives Sanitized Desktop State, bounded sanitized views, or the Recovery Key after the explicit **View** action. It has no generic transport command or direct provider, filesystem, Keychain, or network access.
4. Rust synchronizes only validated cumulative Usage Snapshots through the official Convex Rust client.
5. Convex validates the live Profile and Active Mac generation, then updates Daily Usage, scores, and Aggregate projections transactionally.
6. Rust sanitizes Convex results before React receives them.

## Native contract and state delivery

The top-level Sanitized Desktop State carries a breaking-change contract version, generation time, and monotonic state revision. TypeScript types and strict runtime validators are generated from the canonical Rust DTOs. An unknown contract version fails closed instead of being interpreted approximately.

The main panel uses a snapshot-oriented native interface: React fetches the current cached snapshot, may request a refresh without waiting for provider I/O, and receives revision notices as invalidation hints. It subscribes before the initial read, coalesces notices, refetches the complete snapshot, and accepts only a higher revision. Missed, duplicated, delayed, or reordered notices cannot replace newer state, and partial state patches never cross Tauri IPC.

Expected provider, parsing, network, and persistence failures appear as sanitized unavailable, stale, retry, or synchronization state. Only closed caller, lifecycle, serialization, contract, and internal-invariant failures reject a command. Large Doomerboards use separate typed, bounded commands rather than inflating the main snapshot; settings, Profile, recovery, updates, and future focused surfaces likewise receive narrow interfaces instead of a generic dispatcher.

The accepted interface and ordering contract are recorded in [ADR 0013](adr/0013-use-snapshot-refresh-and-revision-notices-for-native-state.md).

Production WebViews deny arbitrary HTTP and WebSocket egress through CSP and receive no filesystem, shell, HTTP, or Keychain plugin capability. Window-specific command allowlists are backed by Rust caller-window checks. Development builds may permit only localhost traffic required by Vite and HMR.

## Local persistence

Rust owns one transactional SQLite database in Application Support. It separates private parser checkpoints and deduplication metadata, sanitized provider/read-model state, effective-dated pricing versions, and a synchronization outbox. Raw provider content is never copied into the database. The Recovery Key, Better Auth session, and opaque installation credential are separate non-synchronizing Keychain items. Provider credentials remain in provider-owned storage and exist in TouchGrassBar memory only while needed. Profile creation stores the Recovery Key without displaying it. Settings State may contain only its real final three characters for the masked field. Profile Settings may request the full stored Recovery Key through one narrow command and hold it in React memory only while the inline field is visible. After reveal, an explicit Copy action may place the key on the macOS clipboard, which remains outside TouchGrassBar's clearing guarantees. This exception is recorded in [ADR 0015](adr/0015-allow-a-deliberate-recovery-key-reveal-in-react.md).

The native core retains 60 UTC Ranking Days of sanitized Daily Usage Aggregates and synchronization deduplication metadata. Provider-private cost-detail indexes retain only the current UTC Ranking Day and the preceding 29 days. The Claude index stores salted frame and message keys, approved token and pricing metadata, and bounded file checkpoints. It does not store transcript content. Model and pricing details are removed after the 30-day cost window. File checkpoints remain only while they can contribute to the 60-day aggregate and trend window. Pricing versions remain while referenced. A Quota Lane may be cached as stale only until its reset, after which its allowance and remaining value are unavailable. Profile creation queues at most the approved 30-day aggregate backfill.

Each aggregate update and Pending Usage Snapshot upsert commits in one SQLite transaction. The outbox contains one latest cumulative revision per Active Mac generation, provider, and Ranking Day; uploads are bounded and idempotent, and acknowledged revisions alone leave the queue. Active Mac transfer permanently abandons the previous generation's pending rows without deleting local history.

SQLite and IPC schemas use explicit forward-only versions. Database migrations are transactional and create a local backup first. An open or migration failure never silently deletes the database or resets revisions: provider utility may continue in memory, while synchronization reports unavailable until the cache and outbox are safely repaired or restored.

## Refresh and backend transport

A single Rust coordinator shows cached state immediately and refreshes after launch, when stale data is opened, on manual request, wake, network recovery, and every five minutes. Refresh work is single-flight and coalesced; failures preserve stale values and back off. A persistent enabled-by-default provider policy filters refresh adapters before provider work starts. A disabled provider remains visible in registry order with unavailable Quota Lanes. TouchGrassBar does not start later refresh or probe work for it. Its Observed Usage and API-Equivalent Cost do not enter Combined totals. Its private local history remains stored. TouchGrassBar reads Claude quota by running `/usage` through the installed Claude CLI in a bounded private terminal. It does not install or change a Claude status-line bridge. It can accept Claude's standard trust prompt for the isolated probe directory, which Claude can record in provider-owned settings. TouchGrassBar stores a private exact-session cleanup marker in that directory so a later run can remove a probe transcript after a crash. It discards terminal output after it reduces the result to the two quota lanes. A refresh that contains only a Codex provider notification, local usage catch-up, or both does not start a Claude quota probe.

Claude quota and Claude Observed Usage are independent observations. A quota failure does not block a new local usage aggregate. The usage scanner reads main and subagent JSONL files with byte, file, traversal, and time limits. It accepts the exact Claude Code `2.1.223` and `2.1.224` schemas that are verified against Agent SDK `0.3.223` and `0.3.224`. Other versions fail closed. It reads approved wrapper, model, token, modifier, and paid-tool metadata only. It resolves `supersedes` before it groups all files by the salted provider message key. It then adds input, cache-creation input, cache-read input, and output once. Invalid or missing counters keep only a proved partial lower bound. The quota probe transcript is excluded while the probe runs and after a cleanup failure.

`bun run debug:claude-usage` runs the same scanner with an isolated private index. It reports scan state, Today, 7-day, and 30-day totals, model-day token categories, price coverage, and catalog fingerprints. It does not report transcript paths, provider message or session identifiers, credentials, or content.

Rust holds the Keychain-backed Better Auth session, exchanges it for a short-lived memory-only Convex JWT, and uses one official Convex Rust client for typed queries, mutations, and subscriptions. There is no generic endpoint-plus-JSON forwarding command. Authentication or Active Mac rejection stops the applicable queue; ordinary network failure retains it for retry.

## Local-first behavior

The trusted native core remains useful when the network or Convex is unavailable. It caches sanitized provider state and Pending Usage Snapshots locally, labels freshness explicitly, and resumes synchronization when connectivity returns and the same Active Mac generation remains authoritative.

Profile and social features may be unavailable offline, but provider limits and locally observed history must not be blocked by backend readiness.

## Process model

The application runs as a menu-bar-only Tauri process with no Dock icon. The tray interaction controls a compact panel, while onboarding and settings use separate windows. Background refresh, SQLite persistence, backend transport, and update checks run in the native process.

The Tauri viability spike validated the implementation path and identified the remaining physical and release gates documented in `docs/spikes/tauri-menubar-viability.md`.

## Release trust

One unprivileged GitHub Actions job validates an exact stable SemVer tag,
membership in `main`, and successful exact-head CI before a protected job can
request release approval. The `macos-release` environment owns signing,
notarization, provisioning, and updater private material. It creates an arm64
draft Release only. The secretless `public-release` environment is a separate
publication authority.

The release build binds the validated tag version into Tauri, signs and
notarizes the app, creates the Tauri updater archive and signature, and then
independently notarizes and staples the DMG. Sanitized receipts contain public
trust facts and artifact digests only. The executable controls and operator
procedure are in [the release runbook](release.md).
