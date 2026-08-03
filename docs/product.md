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

My Tokenmaxxers is synchronized with the owner's identity so it follows an identity recovery. Removing someone only changes the owner's set and requires no notification or approval.

## Product surfaces

The macOS app owns all interactive product behavior: onboarding, provider status, usage history, Doomerboards, identity recovery, My Tokenmaxxers management, and settings.

The landing site is a static marketing and distribution surface containing product explanation, screenshots, supported providers, the privacy boundary, installation links, and a GitHub link. It exposes no live Doomerboard, Tokenmaxxer profile, identity recovery, My Tokenmaxxers management, or authentication.

## Ranking score

Doomerboards order Tokenmaxxers only by observed tokens. A Provider Doomerboard totals one provider; a Combined Doomerboard adds Codex and Claude tokens without weighting for model, price, plan, or quota percentage.

Equal Token Scores share the same rank. TouchGrass ID provides deterministic display order inside a tie but does not imply a better position.

API-equivalent cost may appear beside token totals as supporting context but never affects rank. It is always marked as approximate and labeled “API equivalent,” for example: `10.2M tokens · ≈ $31.40 API equivalent`. It must never be presented as money spent or billed. Neither number represents productivity or work quality.

An estimate uses an immutable, effective-dated pricing-catalog version applicable on the usage date and records that basis. A semantic catalog change recomputes only affected model-days still retained locally and records the new basis; it never changes Observed Tokens or rank. If any required model or token-category price is unknown, TouchGrassBar shows the tokens without a monetary estimate and never guesses, substitutes another rate, or presents a misleading partial total.

Doomerboard periods use UTC boundaries. “Today” is the current UTC day; 7-day and 30-day rankings include the current Ranking Day and the preceding 6 or 29 complete Ranking Days. Every Tokenmaxxer is therefore measured over the same interval. Provider quota reset times remain provider-defined and are displayed in the Mac's local time.

## Provider limits

TouchGrassBar mirrors every active limit reported by Codex or Claude using that provider's own lanes, labels, units, remaining values, and reset times. It does not collapse multiple windows, normalize limits across providers, or derive a combined quota.

Provider limits, historical Token Scores, and API-equivalent cost are independent displays. No value is converted into another.

A full provider observation is required to initialize a Quota Snapshot. A sparse provider notification may update an existing snapshot but cannot create one. A successfully observed Quota Lane is current for five minutes, then remains visible as stale until its reset; once the reset passes without a refresh, its previous allowance and remaining value become unavailable. A failed refresh or Mac wake preserves the last valid stale value rather than turning it into zero.

## Usage evidence and corrections

Observed Usage records three independent facts: its evidence basis is provider-reported or locally derived; its coverage is complete or partial; and current-day availability is current, stale, or unavailable. Complete means the selected source proves its declared provider scope and supported token categories through the observation time. Any known gap makes it partial. Partial Observed Tokens still contribute their known value to Token Score, with no imputation. Missing usage is unavailable, never zero or estimated. Completed historical days remain available with their recorded coverage rather than becoming stale with age.

Exactly one source owns each provider and Ranking Day. A provider-account daily total wins when available. Local parsing is fallback or model/category detail and is never summed with that total. Codex account daily tokens are used directly; local Codex fallback uses cumulative deltas without adding cached-input or reasoning breakdowns again. Claude sums input, cache-creation input, cache-read input, and output, with thinking already included in output. Unknown schemas fail closed as partial or unavailable.

A Usage Snapshot may replace an earlier provider/day value only with a higher revision. Decreases require explicit stronger evidence from a provider replacement or parser correction; disappearing local logs never reduce a synchronized total. An accepted correction updates the daily total and its derived ranking state together. The revision and reason remain auditable, but “corrected” is not a lasting status or permanent public badge.

Only the provider/day aggregate, evidence basis, coverage, observation time, revision, and complete API-equivalent cost with its pricing basis may synchronize. Raw logs, provider message or session identifiers, credentials, and file paths remain on the Mac.

## Offline behavior

Provider detection, Quota Snapshots, local usage history, and API-Equivalent Cost remain usable without Convex. Identity creation, synchronization, My Tokenmaxxers, and Doomerboards report an honest unavailable or stale state until connectivity returns.

Offline operation is temporary local-first behavior, not a private identity mode. Pending Daily Usage Aggregates synchronize after connectivity returns and only while the Mac remains the identity's Active Mac.

TouchGrassBar retains 60 UTC Ranking Days of local aggregate history for corrections and pricing updates. Creating an identity queues only the approved 30-day backfill. Pending Usage Snapshots are bound to the current Active Mac generation; transfer abandons the previous generation's queue without deleting its device-private history.

## Public by design

Every Tokenmaxxer appears in public Doomerboards; there is no private-account or ranking-opt-out mode. Adding someone to My Tokenmaxxers does not grant access to otherwise private usage—it only filters already-public ranking data into a personal audience.

Public-by-design applies only to Tokenmaxxer identity and Daily Usage Aggregates. Prompts, conversations, credentials, cookies, raw logs, and local file paths remain device-private and prohibited from synchronization.

## Identity and recovery

TouchGrassBar creates an identity without asking for an email address, social login, or user-selected password. The Tokenmaxxer receives a permanent public TouchGrass ID and a generated private Recovery Key. They may choose and later edit a non-unique public Display Name.

Doomerboard rows show the Display Name and TouchGrass ID together, for example `Fabien #TG-7K4P9D`. Adding a Tokenmaxxer and identity recovery use the permanent TouchGrass ID, never the Display Name.

The Recovery Key is stored in macOS Keychain and can restore the identity on another Mac when paired with the TouchGrass ID. Successful recovery requires online confirmation, rotates the Recovery Key, and transfers Active Mac authority. An authenticated Active Mac may securely reveal or rotate its stored key; if the Tokenmaxxer loses both Active Mac access and the current Recovery Key, the identity is permanently unrecoverable. TouchGrassBar provides no alternative account-recovery channel.

Only one Active Mac may synchronize an identity at a time. Authority belongs to an opaque TouchGrassBar installation rather than a hardware fingerprint, so an update or reinstall that preserves Keychain remains the same Active Mac. Restoring elsewhere transfers synchronization authority and invalidates every previous session. Multi-device merging and device-management UI are outside the MVP.

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
