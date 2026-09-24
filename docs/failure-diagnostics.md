# Failure Diagnostics

Use this support interface to inspect received failures without access to a
Tokenmaxxer's Mac. The module implements
[ADR-0021](adr/0021-send-bounded-failure-diagnostics.md).

## Read reports

Start with the TouchGrass ID shown in the app. From the repository root:

```sh
bun run --cwd packages/backend convex run diagnostics:forTouchGrassId \
  '{"touchGrassId":"TG-AAAAAA","paginationOpts":{"numItems":20,"cursor":null}}' \
  --typecheck disable --codegen disable
```

This command reads the deployment selected by the root `.env.local`. Normal
development uses the isolated local deployment. For an authorized production
investigation, add `--deployment <authorized-deployment>` explicitly. Do not
change `.env.local` to select production.

The response contains a `page`, `isDone`, and `continueCursor`. To read the
next page, pass `continueCursor` as `paginationOpts.cursor`. Other internal
queries support device, provider, and failure-group lookup:

- `diagnostics:forDevice` takes `deviceId` and `paginationOpts`.
- `diagnostics:forProvider` takes `provider` and `paginationOpts`; `null`
  selects application-wide reports.
- `diagnostics:forGroup` takes `groupKey` and `paginationOpts`.

These queries require deployment administrator access. They are unavailable
through public client queries and Doomerboards. Do not copy report data into
public issues. Use a minimal description and synthetic reproduction data.

## Capture triggers

| Area            | Report trigger                                                                           | States that stay quiet                                                                      |
| --------------- | ---------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| Database        | Open, migration, invariant, or a supported runtime storage operation fails.              | Successful open or migration.                                                               |
| Parser          | A source read fails, or a complete usage record has an invalid or unsupported shape.     | No source, initial indexing, an unfinished trailing record, or an unreviewed version alone. |
| Pricing         | A calculation fails, or an existing cost is removed because its catalog is not approved. | No usage or low coverage alone.                                                             |
| Provider access | An attempted quota or account-usage request fails or has an invalid response.            | Provider absent, provider disabled, or normal cancellation.                                 |
| Sync            | Delivery fails, a response is invalid, or an exact usage revision conflicts.             | No pending work, normal Active Mac transfer, or a successful session refresh.               |

A later success does not send a recovery report. The worker retries only
queued failures. A report credential must have been registered during an
earlier healthy Profile session to report a database failure remotely.

## Interpret a report

For a Codex or Claude parser or pricing failure, updated clients can include
`context.scan`. It contains the completed scan attempt's status, the running
parser version, the last committed aggregate parser version, the pricing
catalog, file counts by state, and at most 30 days of stored token and cost
evidence. A missing scan field means unknown, including for older reports.

`scan_incomplete` is a failed scan that cannot finish. It can report a retained
file error even when no record is parsed again. For Claude, `files.error > 0` blocks the
complete-scan gate that accepts replacement costs. Codex can retain partial
pricing evidence. Its optional `files.deferred` and `files.excluded` counts
show deferred processing and excluded usage; these counts can overlap other states. `files.indexing` shows
unfinished files. `files.olderParser` includes retained files from earlier
parsers; missing files can remain in this count. File counts cover the whole
index, not one usage day. For Claude, a lower aggregate parser version means that the
current parser has not completed its aggregate update. Codex does not store
that marker or a daily scan revision; those fields are null. These fields explain
the captured attempt; they do not prove the present device state.

Read `report.failure.area`, `code`, and the typed `context` first. Check
`report.firstOccurredAt`, `lastOccurredAt`, and `contextCapturedAt` before
the server `receivedAt`. The timestamps are UTC epoch milliseconds. A report
received now can describe a failure from an earlier day.

`report.app.version` is the app version at failure time. A parser or catalog
version in the context describes the relevant operation or evidence. A null
version is unknown. Do not infer it from the latest release or receipt time.

For Claude parser reports from the updated client, `context.rankingDay` is
the affected UTC usage day, when its timestamp can be read. `recordsAffected`
counts records with that reason on that day during the captured scan;
`recordsExcluded` counts those records whose tokens were excluded. An error
can have zero excluded records when known counters still contribute partially.
`filesSeen`, `recordsAccepted`, `recordsRejected`, and `sourceVersions` describe
the whole scan. Do not attribute those scan totals to one error or day. Do not
add record counts across reports as if they were unique records: rescans can
read the same record again. Repeated occurrences retain the latest captured
context; `occurrenceCount` is separate from these record counts.

The new fields are optional protocol extensions. Older clients and retained
reports remain valid without them. A missing date is unknown, not today's date.
Malformed timestamps cannot be assigned to a usage day. The backend must accept
these fields before an updated desktop client uploads them.

For missing API-Equivalent Cost:

1. Check parser reports for a rejected usage shape or read failure.
2. Check pricing reports for the defined calculation reason and catalog.
3. A `pricing_catalog_not_approved` report shows a local cost that was
   deliberately removed from outgoing usage because its basis was rejected.
4. If a Ranking Day and revision are present, compare that exact evidence
   with the device's `usageBuckets` record. A newer server revision is not
   proof that the older report was incorrect.

For a database failure, inspect the known module versions, stage, and backup
state. `present` does not mean a backup passed validation. Unknown observed
versions mean that the check could not establish a value. Reports never
contain a database dump and cannot authorize a repair.

`occurrenceCount` counts combined failure occurrences before the report was
frozen. An upload retry does not increase it. Similar failures can have
multiple immutable reports. Use `groupKey` to compare them.

## Evidence limits

A successful operation sends nothing. No reports can mean no captured
failure, an older app, first registration not complete, inaccessible
Keychain, offline delivery, expired reports, or revoked report authority.
The bounded memory handoff can also discard reports when full or if the app
exits before the worker saves them. The disk queue removes old entries when
its size limit is reached. This is a support signal, not an exhaustive log.
Silence does not prove current health or recovery.

Normal usage data can separately prove that one provider/day value is now
present. There is no automatic recovery report and no remote command that
reads arbitrary files, logs, or SQL.

The local queue expires reports after seven days and has a 2 MiB storage
budget, including atomic replacement. Server records expire 14 days after
receipt. An hourly internal cleanup deletes expired records in bounded
batches. Diagnostics are independent of normal usage synchronization.

## Local support report

In Settings → General → Support, select **Copy support report**. This reads
bounded scan evidence for every compiled provider (Codex and Claude) from the local database. Paste the
report into the support conversation. It is not uploaded automatically.
The report contains the app version and capture time, but no Profile
credentials, source paths, provider record identifiers, or conversations.

A manual read uses `status: unknown`: stored file states cannot prove that
the last source-directory traversal completed. The file counts, aggregate
parser version, and daily pricing values are still available. The command
uses a read-only connection, one consistent read transaction, a short lock
timeout, and a query deadline. It does not scan transcripts or change the
database. Each entry in `providers` names its provider and scan evidence. An unreadable
provider has `scan: null`; the other provider can still be read. Failure to
open the database returns an error. No read substitutes invented zeroes.

## Remote support

The person selects **Allow remote support for 30 minutes** in Settings →
General → Support. This approves requests for the same bounded report for
all providers. They can select **End remote support** at any time. Quitting
or restarting the app clears local approval. The app must be running and
online, with a valid Profile session and access to its Keychain.

Use the TouchGrass ID and your operator name to request a report:

```sh
bun run --cwd packages/backend convex run support:requestReport \
  '{"touchGrassId":"TG-AAAAAA","operator":"support-operator"}' \
  --typecheck disable --codegen disable
```

Then read the session and its ten latest requests:

```sh
bun run --cwd packages/backend convex run support:forTouchGrassId \
  '{"touchGrassId":"TG-AAAAAA"}' \
  --typecheck disable --codegen disable
```

As with failure reports, these commands use the root local deployment by
default. Add `--deployment <authorized-deployment>` only for an authorized
production session. Both support commands require deployment administrator
access. Public clients cannot request or read another person's report.

The desktop checks for requests about every ten seconds while approval is
active. Each request has a two-minute deadline, limited by session expiry.
Repeated requests return the existing pending request. New requests are at
least 30 seconds apart. The result contains `completedAt` and either `report`
or `failure: report_unavailable`. A null `completedAt` means no reply has
arrived; after `deadline`, request again. It does not prove the Mac is online.

Every poll and result requires current Profile, installation, and Active Mac
generation authority. Expiry, cancellation, device revocation, and recovery
stop access. Local cancellation stops delivery even when the backend is
unreachable. Requests record the administrator-supplied operator label and
request, deadline, and completion times. Completed results cannot change.
Reports expire seven days after their request; session audit records expire
seven days after session expiry. An hourly job removes them in bounded batches.

This operation reads stored scan and pricing state. It does not rescan source
files or repair the Mac. Approved repair actions are tracked in
[#110](https://github.com/FabienGreard/TouchGrassBar/issues/110).

## Release order

Deploy the backend validator that accepts the optional `scan` field and
`scan_incomplete` reason, plus the support-session functions and tables,
before publishing this desktop version. Existing
reports remain valid. Verify an updated client's failed scan arrives with
file counts and daily pricing evidence. A successful backend deployment
alone does not prove that the desktop report was delivered.

## Verification

Shared sanitized fixtures in
`packages/contracts/fixtures/diagnostic-reports-v1.json` must pass both the
TypeScript validator and native serialization checks. Native tests cover
queue persistence, immutable retries, size and age limits, and failure-only
capture. Backend tests cover credential authority, device revocation,
duplicate submissions, rate limits, private lookup, and retention.

A real local HTTP canary must also prove authenticated registration,
submission without a product session, an identical retry, and rejection of
an invalid diagnostic credential. Production verification is a separate
deployment step and requires authorization for that target.
