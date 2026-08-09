# Separate Pending Usage synchronization from Profile provisioning

## Status

Accepted

## Context

Issue #26 synchronizes a Pending Usage Snapshot for the current UTC Ranking
Day. Profile provisioning supplies the live Active Mac authority and secret
material that protected delivery needs. Provider observation supplies each
new Usage Snapshot. These concerns have different retry, pause, and failure
rules.

One shared worker would make callers coordinate Profile provisioning, Active
Mac Generation selection, outbox reads, and batch delivery. Callers would also
apply acknowledgements and publish a safe synchronization status. A generic
event bus would hide these rules and would let unrelated Sanitized Desktop
State Revision Notices start synchronization.

## Decision

Pending Usage Snapshot synchronization is a distinct deep Module. The Profile
Module keeps Profile creation, restoration, Active Mac provisioning, and all
secret custody. Provider observation remains separate and never performs
protected delivery.

The external synchronization Interface has two operations:

- a cause-free `request()` operation; and
- an independent update-pause operation that stops new attempts and waits for
  the active attempt to finish.

The Module also has one internal Profile-facing operation.
`install_authority` receives the server Generation and activation time. It
captures a sanitized baseline only when the observation matches the activation
time. It ignores earlier totals. The first later observation becomes a partial
baseline. This operation is not available to app callers.

The app-facing Interface does not expose Active Mac Generation activation,
Pending Usage Snapshot selection, outbox rows, transport outcomes,
acknowledgements, retry state, or synchronization-status transitions. A narrow
local-state Adapter joins the Module to the Sanitized Desktop State projection.
This gives app callers high Leverage and keeps delivery rules local to one
Module.

App launch and a committed Pending Usage Snapshot can call `request()`
directly. New or restored Active Mac authority and network recovery can also
call it. App foreground, operating-system resume, the explicit **Refresh now**
action, update resume, and the five-minute retry timer can also call it. The
Module does not subscribe to every Sanitized Desktop State Revision Notice. It
does not use a generic event bus.

Synchronization is single-flight. A request during an active attempt records
one rerun. More requests before that rerun do not add more attempts. Update
pause has its own admission state. Update resume records a new request if work
can continue.

Each Coding Provider can commit and request delivery independently. A
completed provider does not wait for another provider. The provider commit
stores its aggregate, correction proof when present, latest cumulative outbox
revision, synchronization status, and Sanitized Desktop State revision in one
SQLite transaction. Acknowledgements and safe status changes also update the
outbox and Sanitized Desktop State atomically. Rust publishes a Revision Notice
only after the transaction commits.

For issue #26, the implementation selects Pending Usage Snapshots only for the
current UTC Ranking Day. Day eligibility is an internal policy. Issue #27 can
expand that policy to the approved current day and previous twenty-nine UTC
days at Profile creation without a change to the external Interface.

## Dependency seams

SQLite queue and correction rules are concrete and internal to the Module.
The Sanitized Desktop State projection owns the outer transaction so its safe
status, revision, aggregate, and outbox stay atomic. The runtime uses a narrow
local-state Adapter for that transaction. Tests use temporary or in-memory
SQLite. The Module does not define a generic repository Interface for SQLite.

Live Profile authority is an in-process internal seam. The production Adapter
gets short-lived authority and secret values from the Profile Module. The test
Adapter supplies deterministic authority. The synchronization Module does not
provision a Profile and does not store the secret values.

Convex delivery is a remote owned internal seam. The production Adapter uses
the protected Convex transport. The test Adapter records requests and returns
deterministic acknowledgements or safe failure outcomes. Both Adapters use the
same synchronization rules behind the external Interface.

## Consequences

Profile failures cannot make provider observation wait. Provider completion
cannot make Profile provisioning wait. Callers cannot apply synchronization
steps in the wrong order. Tests can drive the same small Interface with real
SQLite and test Adapters.

The Module implementation must coordinate SQLite, Profile authority, Convex
delivery, update pause, and Revision Notices. This complexity stays behind the
Interface. A future day-range change or transport change does not require new
caller sequencing.

Server transfer can finish before the local Profile runtime installs the new
authority. A provider observation in this interval cannot supply an exact
activation baseline. The synchronization Module does not count that
observation as the baseline. It keeps the later segment partial.
