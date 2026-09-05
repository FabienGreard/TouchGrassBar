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

React cannot read provider source material, cookies, raw logs, local paths, Convex session material, or local storage. Credentials remain native-owned, except that Profile Settings may receive the stored Recovery Key through one narrow command after the Tokenmaxxer selects **View**, and the shared recovery dialog may hold the entered Recovery Key in volatile component state until it sends one narrow recovery intent. React receives versioned Sanitized Desktop State and bounded view data for all other presentation. Release WebViews are network-dark; development may allow only the localhost Vite/HMR connection.

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

The backend rejects raw provider material and accepts only validated cumulative daily snapshots from the current Active Mac. Convex calculates all daily totals, combined scores, ranks, and public projections. The Doomerboard uses one namespaced Aggregate component; My Tokenmaxxers uses bounded indexed reads and in-memory sorting. A rate limiter protects synchronization, migrations own repairs, and a daily UTC cron expires rolling windows.

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
2. DTOs in the sanitized contract, the deliberate Profile Settings Recovery Key reveal, and the entered Profile recovery credentials may serialize across narrow Tauri commands. Privileged provider credentials and session types are separate and non-serializable through commands.
3. React sends narrow typed intents and receives Sanitized Desktop State, bounded sanitized views, or the stored Recovery Key after the explicit **View** action. The recovery dialog holds its entered credentials only in volatile component state. React has no generic transport command or direct provider, filesystem, Keychain, or network access.
4. Rust synchronizes only validated cumulative Usage Snapshots through the official Convex Rust client.
5. Convex validates the live Profile and Active Mac generation, synchronizes the monotonic enabled-provider setting, then updates Daily Usage, filtered scores, and Aggregate projections transactionally.
6. Rust sanitizes Convex results before React receives them.

## Native contract and state delivery

The top-level Sanitized Desktop State carries a breaking-change contract version, generation time, and monotonic state revision. TypeScript types and strict runtime validators are generated from the canonical Rust DTOs. An unknown contract version fails closed instead of being interpreted approximately.

The main panel uses a snapshot-oriented native interface: React fetches the current cached snapshot, may request a refresh without waiting for provider I/O, and receives revision notices as invalidation hints. It subscribes before the initial read, coalesces notices, refetches the complete snapshot, and accepts only a higher revision. Missed, duplicated, delayed, or reordered notices cannot replace newer state, and partial state patches never cross Tauri IPC.

Rust also derives Overall Quota Headroom from the same committed cached revision. The cached projection keeps the enabled-provider set with that revision. It reduces each enabled provider's active Quota Lanes and then calculates the equal-weighted mean. The menu-bar presenter makes one icon, hover label, and accessibility label from that revision. It subscribes before its initial read, accepts only a higher revision, and does not replace the native image when the visible result is unchanged. The status-item interaction path does not read a provider or the disk. A debug-only `TOUCHGRASS_MENU_BAR_FIXTURE` value can select `unavailable`, `current-0`, `current-34`, `current-100`, or `sequence` for physical checks. Run `TOUCHGRASS_MENU_BAR_FIXTURE=sequence bun run dev:desktop`, then select **Refresh now** to move to the next state. An unknown fixture value, or a fixture without an isolated development instance, stops startup. The fixture uses the production presenter and native status-item adapter, omits the development title, and does not use provider data.

Expected provider, parsing, network, and persistence failures appear as sanitized unavailable, stale, retry, or synchronization state. Only closed caller, lifecycle, serialization, contract, and internal-invariant failures reject a command. Large Doomerboards use separate typed, bounded commands rather than inflating the main snapshot; settings, Profile, recovery, updates, and future focused surfaces likewise receive narrow interfaces instead of a generic dispatcher.

The accepted interface and ordering contract are recorded in [ADR 0013](adr/0013-use-snapshot-refresh-and-revision-notices-for-native-state.md).

Production WebViews deny arbitrary HTTP and WebSocket egress through CSP and receive no filesystem, shell, HTTP, or Keychain plugin capability. Window-specific command allowlists are backed by Rust caller-window checks. Development builds may permit only localhost traffic required by Vite and HMR.

## Local persistence

Rust owns one transactional SQLite database in Application Support. It separates private parser checkpoints and deduplication metadata, sanitized provider/read-model state, effective-dated pricing versions, and a synchronization outbox. Raw provider content is never copied into the database. The Recovery Key, Better Auth session, and opaque installation credential are separate non-synchronizing Keychain items. Provider credentials remain in provider-owned storage and exist in TouchGrassBar memory only while needed. Profile creation stores the Recovery Key without displaying it. Settings State may contain only its real final three characters for the masked field. Profile Settings may request the full stored Recovery Key through one narrow command and hold it in React memory only while the inline field is visible. After reveal, an explicit Copy action may place the key on the macOS clipboard, which remains outside TouchGrassBar's clearing guarantees. This exception is recorded in [ADR 0015](adr/0015-allow-a-deliberate-recovery-key-reveal-in-react.md). The shared recovery dialog may hold the entered Recovery Key in volatile React state until it sends the narrow recovery command. It does not persist, log, preview, or include the value in evidence. This exception is recorded in [ADR 0019](adr/0019-use-the-shared-react-profile-recovery-dialog.md).

The native core retains 60 UTC Ranking Days of sanitized Daily Usage Aggregates and synchronization deduplication metadata. The Codex provider account cache uses the same 60-day UTC window and does not store future account buckets. Provider-private cost-detail indexes retain only the current UTC Ranking Day and the preceding 29 days. The Claude index stores salted frame and message keys, approved token and pricing metadata, and bounded file checkpoints. It does not store transcript content. Model and pricing details are removed after the 30-day cost window. File checkpoints remain only while they can contribute to the 60-day aggregate and trend window. Pricing versions remain while referenced. A stale Quota Snapshot retains every last-known Quota Lane until the next full provider report replaces it. At reset, the old lane leaves the active headroom set, but its allowance, remaining value, and reset time remain in the sanitized snapshot. Profile creation queues at most the approved 30-day aggregate backfill.

Each aggregate update and Pending Usage Snapshot upsert commits in one SQLite transaction. The outbox contains one latest cumulative revision per Active Mac generation, provider, and Ranking Day; uploads are bounded and idempotent, and acknowledged revisions alone leave the queue. Active Mac transfer permanently abandons the previous generation's pending rows without deleting local history.

SQLite and IPC schemas use explicit forward-only versions. One database coordinator inspects the complete SQLite format without a write, rejects an unknown or newer format, creates and verifies one durable backup, runs registered module migrations in a deterministic order, and checks structural and domain invariants. It issues an opaque Ready token only after every check succeeds. Native persistence, provider work, synchronization, and update checks require that token. An open or migration failure stops those operations and never deletes, resets, or partly accepts the database. Every official release keeps one sanitized database fixture. The release gate upgrades and reopens every fixture with the exact candidate code and records the result in release evidence. [ADR 0017](adr/0017-coordinate-forward-sqlite-compatibility.md) records this contract.

## Pending Usage Snapshot synchronization

Pending Usage Snapshot synchronization is a distinct deep Rust Module. Its
external Interface has a cause-free `request()` operation and an independent
update pause. The Profile Module keeps provisioning and secret custody. The
synchronization Module gets live Active Mac authority through production and
test Adapters. Convex delivery also has production and test Adapters. SQLite is
a concrete internal seam and has no generic repository Interface. A narrow
local-state Adapter lets the Sanitized Desktop State projection keep its safe
status, revision, aggregate, and outbox in one outer transaction.

These events can request synchronization:

- app launch;
- a committed Pending Usage Snapshot;
- new or restored Active Mac authority;
- network recovery;
- app foreground;
- operating-system resume;
- the explicit **Refresh now** action;
- update resume;
- the five-minute retry timer.

The Module does not use every Sanitized Desktop State Revision Notice as a
wake source. It does not use a generic event bus. Work is single-flight.
Requests during an attempt produce one rerun.

Each Coding Provider can request delivery as soon as its own commit completes.
It does not wait for another provider. Outbox changes, safe synchronization
status, and the Sanitized Desktop State revision commit atomically. The current
UTC Ranking Day is normally eligible. On first Profile creation, generation one
also queues each derivable provider day from the creation Ranking Day and the
preceding 29 UTC days. This atomic batch includes derivable history for a
disabled provider because the enabled-provider setting controls scores, not
fact retention. An explicit creation-day marker completes an empty or sparse
backfill. Missing days then stay missing. Later historical writes require a
higher revision for an existing day. One exception lets a row first observed
as current after the creation day retry after its UTC day closes when it waited
behind the atomic Profile batch. One bounded transfer-day carryover can preserve
partial coverage after an Active Mac change. The external Interface does not
change. This decision is recorded in
[ADR 0018](adr/0018-separate-pending-usage-synchronization-from-profile-provisioning.md).

## Refresh and backend transport

A separate Rust provider-refresh coordinator shows cached state immediately and refreshes after launch, when stale data is opened, on manual request, wake, network recovery, and every five minutes. Refresh work is single-flight and coalesced; failures preserve stale values and back off. A persistent enabled-by-default provider policy filters refresh adapters before provider work starts. A disabled provider remains visible in registry order with unavailable Quota Lanes. TouchGrassBar does not start later refresh or probe work for it. Its Provider Quota Headroom does not enter Overall Quota Headroom or make the result incomplete. Its Observed Usage and API-Equivalent Cost do not enter Combined totals. Its private local history remains stored. TouchGrassBar reads Claude quota by running `/usage` through the installed Claude CLI in a bounded private terminal. It does not install or change a Claude status-line bridge. It can accept Claude's standard trust prompt for the isolated probe directory, which Claude can record in provider-owned settings. TouchGrassBar stores a private exact-session cleanup marker in that directory so a later run can remove a probe transcript after a crash. It identifies each quota candidate and horizon from its own shape: a percentage counter immediately followed by the reset clause that belongs to it. Heading text is not a parse gate. When a plan renders both provider-wide and model-specific weekly candidates with the same shape, the compacted all-model marker selects the provider-wide candidate. A missing or renamed marker falls back to shape. A window it cannot read leaves its own lane out instead of discarding the window it could read. It discards terminal output after it reduces the result to those quota lanes. A refresh that contains only a Codex provider notification, local usage catch-up, or both does not start a Claude quota probe.

Claude quota and Claude Observed Usage are independent observations. A quota
failure does not block a new local usage aggregate. The usage scanner reads
main and subagent JSONL files with byte, file, traversal, and time limits.

The scanner records eight reviewed Claude Code versions: `2.1.223`, `2.1.224`,
`2.1.236`, `2.1.241`, `2.1.258`, `2.1.259`, `2.1.260`, and `2.1.261`.
The [pricing runbook](../apps/desktop/src-tauri/pricing/README.md) records the
package and fixture evidence. The reviewed set does not gate parsing.
A record from another version with a reviewed shape keeps its known Observed
Tokens and leaves its Ranking Day partial and unpriced. A reviewed-version
record with an unreviewed shape also keeps only its known top-level counters and
leaves the day partial and unpriced. Only a record whose version and usage shape
are both unreviewed withholds its counters. [ADR 0020](adr/0020-audit-coding-provider-contracts-against-reviewed-snapshots.md)
records why identity does not gate observation.

The scanner accepts one message iteration when its counters equal the top-level
counters. It also accepts one exact `thinking_tokens` value when it is not more
than `output_tokens`. These fields are breakdowns. The scanner does not add them
to the top-level counters. Non-null `fallback_credit`, mismatched iterations,
and other output-token detail shapes make a record partial and unpriced.

The scanner can ignore a synthetic API-error record in either reviewed shape:
the earlier shape that carries no HTTP status, error details, or request
identifier, and the later shape that carries an HTTP error status, non-empty
error details, and a non-empty request identifier. Its wrapper, message,
content, and zero counters must match the reviewed shape, and its extended usage
fields must be null. A different API-error shape fails closed.

The scanner reads approved wrapper, model, token, modifier, and paid-tool
metadata only. It resolves `supersedes` before it groups files by the salted
provider message key. It then adds input, cache-creation input, cache-read
input, and output once. Invalid or missing counters keep only a proved partial
lower bound. The scanner excludes the quota probe transcript during a probe
and after a cleanup failure.

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
