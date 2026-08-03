# Gate production Convex readiness with reproducible evidence

TouchGrassBar treats Convex launch readiness as a binary, reproducible contract rather than a review score or an inference from local tests. All mandatory authorization, Active Mac, synchronization, UTC rollover, score/Aggregate consistency, bounded-query, abuse, migration, and generated-credential suites must pass. The exact production deployment must then pass one disposable authenticated end-to-end canary and a production health check before public visibility.

One machine-readable Backend Readiness Evidence artifact binds those results to the Git commit, dependency lock, schema and Board Key versions, policy version, and deployment identity. A relevant change makes the artifact stale. Failed, skipped, or stale mandatory evidence blocks launch; security, privacy, authorization, data-integrity, migration, and canary failures cannot be waived. Pre-traffic production evidence is labeled `canary-only` rather than presented as real-traffic proof.

This keeps the QA contract mostly automated and auditable while preserving the authority boundary between defining readiness, executing production checks, and approving launch. It adds a production canary, consistency tooling, migration rehearsal, and evidence-generation work, but this decision neither executes final QA nor claims that the current backend is ready.
