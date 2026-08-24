# Production Backend Readiness

Backend Readiness Evidence can prove one source commit and one production
Convex deployment as `canary-ready`. It does not prove production readiness,
public traffic, desktop release approval, or a later deployment.

## Authority boundary

The `Governed production backend readiness` workflow is manual. Its production
job uses the `production-backend` GitHub environment. Configure this environment
with required reviewers before the first run.

The local preflight does not use production credentials. The production job can
start only when the preflight passed for the same commit, lock file, schema,
Board Key version, and policy version. Only an approved workflow run can set the
runtime binding, deploy the backend, run the canary, and read production health.

Do not run the binding, deployment, or production command from a normal
development shell. Do not use a development or preview deploy key.

## Required production configuration

Store these values in the `production-backend` environment:

- secret `CONVEX_DEPLOY_KEY`: an exact `prod:<deployment>|...` deploy key;
- variable `TOUCHGRASS_PRODUCTION_DEPLOYMENT`: the exact deployment name;
- variable `TOUCHGRASS_PRODUCTION_CONVEX_URL`: the exact HTTPS
  `convex.cloud` URL; and
- variable `TOUCHGRASS_PRODUCTION_SITE_URL`: the exact HTTPS `convex.site`
  URL.

The backend must already have `BETTER_AUTH_SECRET`. Evidence records only its
presence. It never records the value.

The tracked Device completion and Doomerboard migrations must also show a
successful, complete status in the migrations component. Run or resume them
only through their approved cleanup plans. The readiness workflow verifies
their status but does not start or repair production migrations.

## Workflow

The preflight job performs these steps:

1. Check out a clean source commit and install the exact lock file.
2. Run lint, type checks, all tests, and all builds.
3. Run the interrupted and resumed migration rehearsal as a separate check.
4. Upload a source-bound preflight receipt.

After environment approval, the production job performs these steps:

1. Reject a failed, skipped, or stale preflight.
2. Generate a commit, lock, schema, policy, Board Key, and deployment binding
   that is compiled into the Convex bundle.
3. Deploy this exact backend.
4. Create a disposable Profile with generated credentials.
5. Prove session and JWT exchange, Active Mac claim, synchronization,
   identical retry, Global and My Tokenmaxxers reads, Active Mac transfer,
   old-Mac rejection, and new-Mac synchronization.
6. Delete all canary application, Aggregate, and Better Auth records.
7. Check required configuration, migration completion, zero leftover canary
   markers, backend errors, and the Public Usage/Aggregate invariant.
8. Upload `backend-readiness.json`.

The production runner verifies that the generated binding file is exactly the
deterministic binding for the approved source and deployment before it runs the
canary or writes evidence.

Canary cleanup is mandatory. A cleanup error makes the canary fail. The cleanup
operation is bounded and idempotent. It removes application, Aggregate, Better
Auth, and rate-limit component state. A short-lived internal marker prevents a
canary Profile from being added to My Tokenmaxxers. Cleanup removes that marker
with the canary Profile. If the runner stops before cleanup, the stored marker
schedules the same bounded cleanup after 30 minutes. Health also requires zero
remaining canary markers.

## Evidence rules

The artifact contains only deployment identity, source hashes, versions,
timestamps, statuses, booleans, and aggregate counts. It does not contain
credentials, sessions, recovery material, Profile identifiers, provider data,
private logs, or private paths.

The artifact is `canary-ready` only when all four mandatory checks passed and
the runtime binding equals the source binding. A failed, skipped, missing,
stale, or incomplete check produces `not-ready` or no artifact.
`trafficEvidence` is always `canary-only` before launch, and
`productionReadiness` remains `not-ready`. Canary-only evidence cannot be
represented as production-ready.

Any later commit, lock file, schema, Board Key, policy, or production deployment
change invalidates the evidence. Run the complete workflow again after such a
change.
