---
status: accepted
---

# Send Bounded Failure Diagnostics

TouchGrassBar sends a structured report when a supported native operation
fails. One report format covers every Coding Provider and application-wide
database failures. Reports explain the failure and the state at that time.
They do not establish current device health.

## Capture

The Rust diagnostic module owns capture, local retention, grouping, and
delivery. Callers provide a typed failure with a fixed reason code. They do
not provide arbitrary log text, SQL, paths, provider record identifiers,
prompts, conversations, or credentials.

Database reports contain known format and module versions, the failed stage,
and backup presence. Diagnostic reads use fixed read-only queries with a
short busy timeout and a query progress deadline. A failed read produces an
unknown value, never zero. Capture does not repair, reset, migrate, or restore
the database.

Parser, pricing, provider-access, and sync reports use the same envelope with
defined context for each area. A provider identifier is required only for a
provider failure. A Ranking Day and revision are included only when known.
Version fields describe the captured operation or original evidence; the
current app version must not be attributed to an old usage revision.

The Failure Report validator is separate from the generated Tauri IPC
contract in `packages/contracts`. Shared sanitized fixtures must pass both
the TypeScript validator and native serialization checks. Reports never
enter the React interface.

Successful starts, migrations, scans, and updates send no report. Disabled
providers, no usage, initial indexing, normal Active Mac transfer, low pricing
coverage alone, and an unreviewed provider version alone are not failures.
There are no health pings, success reports, or automatic recovery reports.

## Local delivery

The bounded diagnostic queue is outside the main SQLite database. It must
remain usable when the database readiness check fails. If durable storage is
unavailable, bounded memory can retain reports for the current process.
Capture and delivery must not block normal product work or create recursive
reports when diagnostic delivery itself fails.

Reports are at most 32 KiB. The local queue is at most 2 MiB and retains
reports for at most seven days. Repeated failures are combined locally. A
report is frozen before its first upload; all retries use the same report ID
and content. Later failures form another report. Retry scheduling sends only
queued failures and does not produce periodic health data.

## Identity and backend

`diagnosticReporters` binds a stable reporter to one Tokenmaxxer, device, and
Active Mac generation. Registration requires normal authenticated Active Mac
authority. A separate Keychain credential permits report submission only.
The server stores its digest. Credential rotation preserves the reporter ID;
device revocation or generation replacement removes its authority.

Diagnostic submission does not require an open main SQLite database. This
exception does not grant usage synchronization, profile recovery, device
activation, data reads, or remote commands. Queued reports must never be
reassigned to another Profile or generation. A first-start failure before
registration, inaccessible Keychain, or a revoked credential can prevent
remote delivery.

`diagnosticReports` stores immutable failure evidence. The server assigns
profile and device references, receipt and expiry times, and payload and
group digests. It accepts an identical retry once, rejects reuse of a report
ID with different content, and limits submission rates. The server retains
reports for 14 days and deletes expired records in bounded batches.

Diagnostic reads are internal support operations. Public Doomerboards and
ordinary client queries do not expose these records. Lookup indexes cover
profile, device, failure group, and expiry. An incident table is not required
for the initial implementation.

Client failure times and server receipt time remain separate. A delayed
report proves a past failure. Silence does not prove that a device is online
or that a failure has stopped. Normal usage data can independently prove
that one specific provider/day value has been restored.

## Existing contracts

This decision extends ADR-0001 to permit bounded operational failure data in
addition to Daily Usage Aggregates. Sensitive source material remains local.
It adds the narrow diagnostic-delivery exception to ADR-0017 and a separate
bounded diagnostic file queue to ADR-0011. The canonical product database,
its migration rules, and normal Active Mac authority remain unchanged.
