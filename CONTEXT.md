# TouchGrassBar

TouchGrassBar helps people understand their AI coding consumption and compare selected usage summaries with other Tokenmaxxers without exposing the underlying work.

## Usage

**Coding Provider**:
An AI coding service whose usage TouchGrassBar can observe. The MVP providers are Codex and Claude.
_Avoid_: AI, model, vendor

**Quota**:
A provider-defined allowance for a bounded period. A quota is not the same as locally observed usage.
_Avoid_: Usage, balance

**Quota Window**:
The provider-defined period over which a quota applies, ending at a reset time.
_Avoid_: Day, billing period

**Quota Lane**:
One provider-reported limit with its provider-defined label, unit, remaining value, and Quota Window. Quota Lanes are presented without cross-provider normalization.
_Avoid_: Token Score, usage total, combined quota

**Quota Snapshot**:
A point-in-time collection of a coding provider's Quota Lanes. It can be initialized only from a full provider report; sparse notifications may update an existing snapshot but cannot create one.
_Avoid_: Usage aggregate, balance

**Usage Evidence Basis**:
Whether Observed Usage is `provider-reported` or `locally-derived`. This describes the selected source, independently of coverage or freshness.
_Avoid_: Accurate, estimated, quota

**Usage Coverage**:
Whether the selected source is `complete` through its observation time or `partial` because a known segment, supported token category, transfer interval, or provider scope is missing. Partial usage contributes only its known Observed Tokens; TouchGrassBar never imputes the gap.
_Avoid_: Freshness, availability, estimated usage

**Usage Availability**:
For the current Ranking Day, whether Observed Usage is `current`, `stale`, or `unavailable`. A successful observation is current for five minutes and then stale while its last-known value remains valid; no valid source is unavailable. A completed historical day remains available and carries its recorded Usage Coverage rather than becoming stale with age.
_Avoid_: Coverage, evidence basis, corrected

**Observed Usage**:
Coding-provider consumption selected from exactly one source for one provider and Ranking Day. Provider-reported usage takes precedence when available; locally derived usage is a fallback or cost-detail source and is never added to the provider total.
_Avoid_: Quota usage, billed usage, exact usage

**Observed Tokens**:
Token throughput counted under provider-specific rules without double-counting overlapping fields. Codex provider daily totals are used directly; its local cumulative fallback does not add cached-input or reasoning breakdowns again. Claude totals input, cache-creation input, cache-read input, and output while treating thinking as part of output. Provider-specific differences in tokenization and reporting remain part of the number.
_Avoid_: Billed tokens, normalized tokens, productivity

**Token Score**:
The unweighted sum of observed tokens for a Tokenmaxxer, time range, and provider scope. It is the sole Doomerboard ordering metric.
_Avoid_: Usage score, productivity score, points

**API-Equivalent Cost**:
An approximate estimate of what observed tokens would cost at the model's published per-token price applicable on the usage date, canonically displayed with `≈` and the label “API equivalent.” It records the immutable, effective-dated pricing-catalog version used. Historical estimates retain that basis unless a catalog correction recomputes affected retained model-days under a new version; an unknown required price produces no estimate rather than a guessed or partial total.
_Avoid_: Spend, bill, actual cost

**Daily Usage Aggregate**:
A summary of Observed Usage for one Coding Provider, one Tokenmaxxer, and one Ranking Day. It contains only the aggregate, Usage Evidence Basis, Usage Coverage, observation time, revision, and a complete API-Equivalent Cost with its pricing basis when available. It is the most detailed usage record that may leave the Tokenmaxxer's Mac.
_Avoid_: Raw usage, usage log

**Usage Snapshot**:
A cumulative Daily Usage Aggregate sent by the Active Mac with a monotonically increasing revision. Equal or older revisions are ignored. A higher revision may correct a synchronized Ranking Day, but a decrease requires an explicit provider replacement or parser correction; a missing local record never subtracts usage. “Corrected” describes the audited change, not a lasting public status.
_Avoid_: Token increment, usage event, raw observation

**Board Key**:
A versioned identifier for one provider scope and rolling window, such as `tokens-v1:combined:30d`. It namespaces global Aggregate data.
_Avoid_: Leaderboard ID, arbitrary cache key

## Social Comparison

**Tokenmaxxer**:
A person whose AI coding activity is tracked by TouchGrassBar and whose Daily Usage Aggregates appear in public Doomerboards.
_Avoid_: Participant, friend, employee, developer, user

**TouchGrass ID**:
A Tokenmaxxer's permanent, server-generated public identifier, used by another Tokenmaxxer to add them or by its owner to restore their identity.
_Avoid_: Account ID, friend code, secret code

**Display Name**:
A Tokenmaxxer's editable, non-unique public label. It is shown with the TouchGrass ID and never replaces that canonical identifier.
_Avoid_: Username, handle, account name

**Recovery Key**:
A Tokenmaxxer's generated private credential used with their TouchGrass ID to restore their identity on another Mac. Successful recovery replaces it with a new key; without both access to the Active Mac and the current key, the identity is unrecoverable.
_Avoid_: Private ID, password, recovery code

**Active Mac**:
The single TouchGrassBar installation currently authorized to synchronize a Tokenmaxxer's usage. Restoring the identity elsewhere transfers that authority; an Active Mac is not identified by a hardware fingerprint.
_Avoid_: Primary device, linked device

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
An ordered view of Tokenmaxxers by Token Score for a time range and provider scope.
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
