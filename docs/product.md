# Product Definition

## Primary users

TouchGrassBar is for people who use Codex or Claude for coding and enjoy comparing their activity with other Tokenmaxxers. Colleagues can add each other as Tokenmaxxers; their shared employer has no meaning in the product. The primary user is motivated by playful competition—tokenmaxxing—not organizational oversight.

Engineering managers and technical leaders may find the rankings interesting, but employee monitoring and productivity management are not product goals.

## Core problem

AI coding activity is mostly invisible and solitary. People can lose track of their own provider limits, and Tokenmaxxers have no lightweight, privacy-preserving way to compare activity with people they know.

TouchGrassBar makes personal limits glanceable and daily usage aggregates public. It must not imply that higher usage means higher productivity, better engineering, or stronger job performance.

## Intended behavior

A Tokenmaxxer should be able to glance at the menu bar to understand current availability, then open the panel to compare recent activity with other Tokenmaxxers. The experience should feel playfully bleak rather than managerial.

Each Tokenmaxxer has a unique public TouchGrass ID. Another Tokenmaxxer can use that ID to add them to My Tokenmaxxers without a request or approval. Adding is unilateral: the added Tokenmaxxer does not automatically add the other Tokenmaxxer back.

My Tokenmaxxers is synchronized with the owner's Profile so it follows a Profile recovery. Removing someone only changes the owner's set and requires no notification or approval.

## Product surfaces

The macOS app owns all interactive product behavior: onboarding, provider status, usage history, Doomerboards, Profile recovery, My Tokenmaxxers management, and settings.

The landing site is a static marketing and distribution surface containing product explanation, screenshots, supported providers, the privacy boundary, installation links, and a GitHub link. It exposes no live Doomerboard, Tokenmaxxer Profile, Profile recovery, My Tokenmaxxers management, or authentication.

## Updates and recovery

TouchGrassBar uses one stable update channel. It checks quietly on the first panel open, at most once in 24 hours, and also gives the Tokenmaxxer a separate **Check now** action in Settings. An available update uses the existing compact download icon in the panel header and the existing version row in Settings. Ignoring the icon leaves it available after restart. Only selecting the icon or the compact **Install & Relaunch** Settings action can download, verify, install, and immediately restart the app. TouchGrassBar does not silently install or restart.

The native core owns update checks and installation. React receives only a bounded update state. After the Tauri signature is valid, the native core pauses and flushes owned work before it replaces the app. The app bundle update does not reset the Rust-owned SQLite database or macOS Keychain items.

A validated minimum version can pause only incompatible online or public work. Local provider utility, Settings, update controls, and DMG recovery stay available. Network, download, signature, interruption, low-disk, permission, or replacement failure gives the Tokenmaxxer **Retry** and **Download latest DMG** without exposing raw failure detail. Recovery moves forward to a higher SemVer. The product does not promise automatic rollback or downgrade.

## Ranking score

Doomerboards order Tokenmaxxers only by observed tokens. A Provider Doomerboard totals one provider; a Combined Doomerboard adds Codex and Claude tokens without weighting for model, price, plan, or quota percentage.

Equal Token Scores share the same rank. TouchGrass ID provides deterministic display order inside a tie but does not imply a better position.

API-equivalent cost may appear beside token totals as supporting context but never affects rank. It is always marked as approximate and labeled “API equivalent,” for example: `10.2M tokens · ≈ $31.40 API equivalent`. It must never be presented as money spent or billed. Neither number represents productivity or work quality.

An estimate uses an immutable, effective-dated pricing-catalog version applicable on the usage date and records that basis. A semantic catalog change recomputes only affected model-days still retained locally and records the new basis; it never changes Observed Tokens or rank. An unknown model or token-category price leaves only that local detail unpriced. When other priced local evidence supplies a defensible average rate, TouchGrassBar can apply that rate to the authoritative token total and report the priced-detail coverage as a Modeled estimate. Without usable priced evidence, it shows the tokens without a monetary estimate.

Doomerboard periods use UTC boundaries. “Today” is the current UTC day; 7-day and 30-day rankings include the current Ranking Day and the preceding 6 or 29 complete Ranking Days. Every Tokenmaxxer is therefore measured over the same interval. Provider quota reset times remain provider-defined and are displayed in the Mac's local time.

## Provider limits

TouchGrassBar mirrors each supported active limit reported by Codex or Claude using that provider's own lanes, labels, units, remaining values, and reset times. It does not collapse or convert those provider-native values. The Codex `GPT-5.3-Codex-Spark` weekly limit is excluded because it is model-specific. The Codex provider card shows the account weekly limit and the account five-hour limit when Codex reports them.

For an at-a-glance summary, Provider Quota Headroom is the lowest remaining percentage among one provider's active Quota Lanes. A lane that genuinely does not exist is ignored, but an active lane without a calculable remaining percentage makes that provider's headroom unavailable rather than allowing another lane to overstate it. Overall Quota Headroom is the equal-weighted mean of the calculable Codex and Claude headroom values: Codex at 8% and Claude at 60% produce 34%. A still-valid stale value remains in the calculation and makes the overall result stale. An unavailable configured provider is excluded and makes the result incomplete rather than contributing zero; if no provider has calculable headroom, Overall Quota Headroom is unavailable. This index does not sum allowances, estimate remaining tokens, or replace the underlying Quota Lanes.

The menu-bar item keeps the approved flower mark intact and places a compact horizontal headroom meter beneath it. The bright portion represents Overall Quota Headroom remaining, so the meter is full at 100% and empty at 0%; the flower itself does not progressively disappear. A current, complete result uses a continuous meter; a stale and/or incomplete result keeps the same truthful length but uses one shared segmented treatment, with the precise condition available in the opened panel. Calculated 0% retains the empty meter track. Unavailable headroom hides the meter entirely and leaves the flower-only item, with both its visible hover label and VoiceOver label set to `TouchGrassBar`. When headroom is calculable, both labels contain only the app name and rounded current percentage, for example `TouchGrassBar — 34%`; they do not announce freshness or completeness.

Provider limits, historical Token Scores, and API-equivalent cost are independent displays. No value is converted into another.

A full provider observation is required to initialize a Quota Snapshot. A sparse provider notification may update an existing snapshot but cannot create one. A successfully observed Quota Lane is current for five minutes, then remains visible as stale until its reset; once the reset passes without a refresh, its previous allowance and remaining value become unavailable. A failed refresh or Mac wake preserves the last valid stale value rather than turning it into zero.

## Usage evidence and corrections

Observed Usage records three independent facts: its evidence basis is provider-reported or locally derived; its coverage is complete or partial; and current-day availability is current, stale, or unavailable. Complete means the selected source proves its declared provider scope and supported token categories through the observation time. Any known gap makes it partial. Partial Observed Tokens still contribute their known value to Token Score, with no imputation. Missing usage is unavailable, never zero or estimated. Completed historical days remain available with their recorded coverage rather than becoming stale with age.

Exactly one source owns each provider and Ranking Day. A provider-account daily total wins when available. Local parsing is fallback or model/category detail and is never summed with that total. Codex account daily tokens are used directly; local Codex fallback uses cumulative deltas without adding cached-input or reasoning breakdowns again. Claude sums input, cache-creation input, cache-read input, and output, with thinking already included in output. Unknown schemas fail closed as partial or unavailable.

A Usage Snapshot may replace an earlier provider/day value only with a higher revision. Decreases require explicit stronger evidence from a provider replacement or parser correction; disappearing local logs never reduce a synchronized total. An accepted correction updates the daily total and its derived ranking state together. The revision and reason remain auditable, but “corrected” is not a lasting status or permanent public badge.

Only the provider/day aggregate, evidence basis, coverage, observation time, revision, and best defensible API-equivalent cost with its pricing basis may synchronize. Raw logs, provider message or session identifiers, credentials, and file paths remain on the Mac.

## Offline behavior

Provider detection, Quota Snapshots, local usage history, and API-Equivalent Cost remain usable without Convex. Profile creation, synchronization, My Tokenmaxxers, and Doomerboards report an honest unavailable or stale state until connectivity returns.

Offline operation is temporary local-first behavior, not a private Profile mode. Pending Daily Usage Aggregates synchronize after connectivity returns and only while the Mac remains the Profile's Active Mac.

TouchGrassBar retains 60 UTC Ranking Days of sanitized aggregate history for corrections and synchronization. Its provider-private rollout cost-detail index retains only the current UTC Ranking Day and the preceding 29 days. A pricing-manifest update reprices those stored details without reading rollout files again. Creating a Profile queues only the approved 30-day backfill. Pending Usage Snapshots are bound to the current Active Mac generation; transfer abandons the previous generation's queue without deleting its device-private history.

## Public by design

Every Tokenmaxxer appears in public Doomerboards; there is no private-account or ranking-opt-out mode. Adding someone to My Tokenmaxxers does not grant access to otherwise private usage—it only filters already-public ranking data into a personal audience.

Public-by-design applies only to the Tokenmaxxer Profile and Daily Usage Aggregates. Prompts, conversations, credentials, cookies, raw logs, and local file paths remain device-private and prohibited from synchronization.

## Profile and recovery

TouchGrassBar creates a Profile without asking for an email address, social login, or user-selected password. The Tokenmaxxer receives a permanent public TouchGrass ID and a generated private Recovery Key. They may choose and later edit a non-unique public Display Name.

Doomerboard rows show the Display Name and TouchGrass ID together, for example `Fabien #TG-7K4P9D`. Adding a Tokenmaxxer and Profile recovery use the permanent TouchGrass ID, never the Display Name.

The Recovery Key is stored in macOS Keychain and can restore the Profile on another Mac when paired with the TouchGrass ID. Successful recovery requires online confirmation, rotates the Recovery Key, and transfers Active Mac authority. An authenticated Active Mac may securely reveal or rotate its stored key; if the Tokenmaxxer loses both Active Mac access and the current Recovery Key, the Profile is permanently unrecoverable. TouchGrassBar provides no alternative account-recovery channel.

Only one Active Mac may synchronize a Profile at a time. Authority belongs to an opaque TouchGrassBar installation rather than a hardware fingerprint, so an update or reinstall that preserves Keychain remains the same Active Mac. Restoring elsewhere transfers synchronization authority and invalidates every previous session. Multi-device merging and device-management UI are outside the MVP.

## Explicit non-goals

- Employee monitoring or productivity scoring
- Manager dashboards or workforce reporting
- Companies, team spaces, organization membership, and organization roles
- Business-to-business administration or reporting
- Private accounts or a ranking visibility setting
- Email, social-login, or user-selected-password onboarding
- Concurrent synchronization from multiple Macs
- Web Doomerboards, public profile pages, or browser-based account management
- Evaluating the quality, value, or content of a Tokenmaxxer's work
- Ranking by price, subscription tier, quota percentage, or a normalized productivity score
- Uploading source material used to derive usage aggregates
