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

Read `report.failure.area`, `code`, and the typed `context` first. Check
`report.firstOccurredAt`, `lastOccurredAt`, and `contextCapturedAt` before
the server `receivedAt`. The timestamps are UTC epoch milliseconds. A report
received now can describe a failure from an earlier day.

`report.app.version` is the app version at failure time. A parser or catalog
version in the context describes the relevant operation or evidence. A null
version is unknown. Do not infer it from the latest release or receipt time.

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
