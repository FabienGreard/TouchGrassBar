# TouchGrassBar

TouchGrassBar helps people understand their AI coding consumption and compare selected usage summaries with other Tokenmaxxers without exposing the underlying work.

## Usage

**Coding Provider**:
An AI coding service whose usage TouchGrassBar can observe. The MVP providers are Codex and Claude.
_Avoid_: AI, model, vendor

**Provider Presence**:
Whether a Coding Provider installation is locally detected, not detected, or cannot be checked. Provider Presence does not prove authentication, Quota access, or Observed Usage availability.
_Avoid_: Provider connection, provider authorization, provider availability

**Provider Enablement**:
Whether a Coding Provider is active in TouchGrassBar. Providers are enabled by default. A disabled provider remains visible in registry order with unavailable Quota Lanes. TouchGrassBar does not start later refresh or probe work for it. Its Provider Quota Headroom does not contribute to Overall Quota Headroom, and it does not make that result incomplete. Its Observed Usage and API-Equivalent Cost do not contribute to Combined totals. Disabling a provider does not delete its local history. Re-enabling it restores the last-known Quota Snapshot, Observed Usage, and API-Equivalent Cost before the fresh read completes, without replacing that cache with a loading state.
_Avoid_: Provider Presence, provider authentication, data deletion

**Quota**:
A provider-defined allowance for a bounded period. A quota is not the same as locally observed usage.
_Avoid_: Usage, balance

**Quota Window**:
The provider-defined period over which a quota applies, ending at a reset time.
_Avoid_: Day, billing period

**Quota Lane**:
One provider-reported limit with its provider-defined label, unit, remaining value, and Quota Window. Its provider-native values remain visible even when its remaining share contributes to Overall Quota Headroom. At reset, the old lane leaves the active set but remains in the stale Quota Snapshot until the provider reports the next window.
The model-specific Codex `GPT-5.3-Codex-Spark` weekly limit and the Codex
`gpt-reserve` fallback limit are not supported Quota Lanes.
_Avoid_: Token Score, usage total, quota headroom

**Provider Quota Headroom**:
The lowest remaining share among one Coding Provider's active Quota Lanes. A genuinely absent lane is ignored, while any active lane whose remaining share is unknown makes the provider's headroom unavailable.
_Avoid_: Provider usage, provider balance

**Overall Quota Headroom**:
The equal-weighted mean of calculable Provider Quota Headroom values across enabled Coding Providers. Current and still-valid stale values may contribute. Any stale contributor makes the result stale. An unavailable enabled provider is excluded and makes the result incomplete rather than contributing zero. TouchGrassBar rounds only the final mean to the nearest whole percentage for its label, with an exact half rounded up. It is an at-a-glance index, not a sum of provider allowances or an estimate of tokens remaining.
_Avoid_: Combined quota, usage left, quota total

**Quota Snapshot**:
A point-in-time collection of a coding provider's Quota Lanes. It can be initialized only from a full provider report; sparse notifications may update an existing snapshot but cannot create one. A restored Quota Snapshot starts stale until the next full provider report. A stale Quota Snapshot retains all last-known Quota Lanes until a new full provider report replaces it. TouchGrassBar presents these last-known values without a freshness notice.
_Avoid_: Usage aggregate, balance

**Usage Evidence Basis**:
Whether one selected daily value is `provider-reported` or `locally-derived`. A period is `mixed` when its selected Ranking Days use both bases, including within one Coding Provider. This describes the selected source, independently of coverage or freshness.
_Avoid_: Accurate, estimated, quota

**Usage Coverage**:
Whether the selected source is `complete` through its observation time or `partial` because a known segment, supported token category, transfer interval, or provider scope is missing. Partial usage contributes only its known Observed Tokens; TouchGrassBar never imputes the gap.
_Avoid_: Freshness, availability, estimated usage

**Usage Availability**:
For the current Ranking Day, whether Observed Usage is `current`, `stale`, or `unavailable`. A successful observation is current for five minutes and then stale while its last-known value remains valid; no valid source is unavailable. A completed historical day remains available and carries its recorded Usage Coverage rather than becoming stale with age.
_Avoid_: Coverage, evidence basis, corrected

**Observed Usage**:
Coding-provider consumption selected from exactly one source for one provider and Ranking Day. Provider-reported usage takes precedence for that day. If the provider has no bucket for the day, valid locally derived usage can be selected as a fallback. The two sources are never added for the same day. If neither source has valid evidence, usage is unavailable. An omitted provider bucket is not proof of zero usage.
_Avoid_: Quota usage, billed usage, exact usage

**Observed Tokens**:
Token throughput counted under provider-specific rules without double-counting overlapping fields. Codex provider daily totals are used directly when present. Its local cumulative data supplies a fallback for a missing provider day and supplies pricing and model evidence. The local calculation does not add cached-input or reasoning subsets again. Claude totals input, cache-creation input, cache-read input, and output while treating thinking as part of output. Provider-specific differences in tokenization and reporting remain part of the number.
_Avoid_: Billed tokens, normalized tokens, productivity

**Usage Trend**:
The percentage change in Observed Tokens between one displayed period and the equal period immediately before it. Both periods use the same per-Ranking-Day source-selection rule. An omitted provider bucket can use valid local evidence; it does not count as zero. A trend is unavailable when either period is mixed or partial. The previous period must contain at least one observed bucket and a non-zero token total. A Combined Usage Trend is weighted by each contributing provider's previous Observed Tokens. API-Equivalent Cost and pricing evidence do not affect Usage Trend.
_Avoid_: Cost trend, spend change, price change

**Token Score**:
The unweighted sum of observed tokens for a Tokenmaxxer, time range, and provider scope. It is the sole Doomerboard ordering metric.
_Avoid_: Usage score, productivity score, points

**API-Equivalent Cost**:
An approximate estimate of what Observed Tokens would cost at published per-token prices applicable on the usage date, canonically displayed with `≈` and the label “API equivalent.” It records the immutable, effective-dated pricing-catalog version used. Reconciled cost uses local priced detail that equals the authoritative tokens. Modeled cost applies a defensible average rate from priced local detail to the authoritative tokens and reports that priced-detail coverage. Local-only cost uses available local detail when provider-reported usage is unavailable. An unknown price leaves only that detail unpriced; a period with other usable priced evidence may still have a Modeled or Local-only estimate, while a period with no defensible priced evidence has no estimate. Combined adds only valid provider estimates and reports priced-token coverage when another contributing provider is unpriced; it never applies one provider's rate to another provider. A catalog correction recomputes only affected retained model-days and never changes Token Score or Doomerboard rank.
_Avoid_: Spend, bill, actual cost

**Daily Usage Aggregate**:
A summary of Observed Usage for one Coding Provider, one Tokenmaxxer, and one Ranking Day. It contains only the aggregate, Usage Evidence Basis, Usage Coverage, observation time, revision, and the best defensible API-Equivalent Cost with its pricing basis when available. It is the most detailed usage record that may leave the Tokenmaxxer's Mac.
_Avoid_: Raw usage, usage log

**Usage Snapshot**:
A cumulative Daily Usage Aggregate sent by the Active Mac with a monotonically increasing revision. Equal or older revisions are ignored. A higher revision may correct a synchronized Ranking Day, but a decrease requires an explicit provider replacement or parser correction; a missing local record never subtracts usage. Provider-reported usage that arrives later replaces a locally derived value for the same Ranking Day, even when the provider total is lower. “Corrected” describes the audited change, not a lasting public status.
_Avoid_: Token increment, usage event, raw observation

**Pending Usage Snapshot**:
The latest Usage Snapshot awaiting acceptance for one Active Mac generation, Coding Provider, and Ranking Day. It may retry only under that generation and is abandoned after Active Mac transfer.
_Avoid_: Upload event, transferable history, token increment

**Board Key**:
A versioned identifier for one provider scope and rolling window, such as `tokens-v1:combined:30d`. It namespaces global Aggregate data.
_Avoid_: Leaderboard ID, arbitrary cache key

**Backend Readiness Evidence**:
A machine-generated, pass/fail artifact bound to one Git commit and one production Convex deployment. It records the exact schema, Board Key, dependency, policy, migration-rehearsal, automated-test, authenticated-canary, and production-health evidence used to decide whether the backend may launch. A relevant change makes the evidence stale; local or development success alone is never Backend Readiness Evidence.
_Avoid_: Readiness score, QA opinion, local green build

## Native Boundary

**Sanitized Desktop State**:
The versioned, revisioned, bounded projection of native-owned product state that may enter the React interface. It contains display-safe provider, Profile, synchronization, and social data but no credentials or provider source material. React replaces it from a complete cached snapshot; revision notices never carry partial state.
_Avoid_: App state, native state, raw snapshot

## Social Comparison

**Tokenmaxxer**:
A person whose AI coding activity is tracked by TouchGrassBar and whose Daily Usage Aggregates appear in public Doomerboards.
_Avoid_: Participant, friend, employee, developer, user

**Profile**:
A Tokenmaxxer's public representation: their editable Display Name paired with their permanent TouchGrass ID. Recovery and Mac transfer preserve the same Profile.
_Avoid_: Identity, account

**TouchGrass ID**:
A Tokenmaxxer's permanent, server-generated public identifier, used by another Tokenmaxxer to add them or by its owner to restore their Profile.
_Avoid_: Account ID, friend code, secret code

**Display Name**:
A Tokenmaxxer's editable, non-unique public label. It is shown with the TouchGrass ID and never replaces that canonical identifier.
_Avoid_: Username, handle, account name

**Recovery Key**:
A Tokenmaxxer's generated private credential used with their TouchGrass ID to restore their Profile on another Mac. Successful recovery replaces it with a new key; without both access to the Active Mac and the current key, the Profile is unrecoverable.
_Avoid_: Private ID, password, recovery code

**Active Mac**:
The single TouchGrassBar installation currently authorized to synchronize a Tokenmaxxer's usage. Restoring the Profile elsewhere transfers that authority; an Active Mac is not identified by a hardware fingerprint.
_Avoid_: Primary device, linked device

**Active Mac Generation**:
The current epoch of an Active Mac's synchronization authority. Recovery advances it, and authority or Pending Usage Snapshots from an earlier generation cannot be reused.
_Avoid_: Device version, session version

**My Tokenmaxxers**:
The Tokenmaxxers that the current Tokenmaxxer has unilaterally added using their TouchGrass IDs. Adding requires no request, acceptance, or reciprocal relationship.
_Avoid_: Friends, rivals, following, connections

**Tokenmaxxing**:
Playful competition around visible AI coding activity. It describes the social behavior, not a claim that more activity means better work.
_Avoid_: Productivity, performance

**Ranking Day**:
A calendar day bounded by midnight UTC. Location and travel do not change its boundaries.
_Avoid_: Local day, usage day

**Doomerboard**:
An ordered view of Tokenmaxxers by Token Score for a time range and provider scope. Each row has a unique rank. Equal Token Scores use TouchGrass ID in ascending order as the stable tie-break.
_Avoid_: Leaderboard, feed, activity stream

**Provider Doomerboard**:
A Doomerboard based on one Coding Provider.
_Avoid_: Provider ranking

**Combined Doomerboard**:
A Doomerboard whose score combines supported Coding Providers.
_Avoid_: Global leaderboard, combined leaderboard, overall provider ranking

**Global**:
The Doomerboard audience containing every Tokenmaxxer.
_Avoid_: Public leaderboard, global audience

**My Tokenmaxxers Audience**:
The Doomerboard audience containing My Tokenmaxxers.
_Avoid_: Friends leaderboard, private leaderboard
