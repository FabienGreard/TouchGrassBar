# SQLite release fixtures

These files are permanent compatibility inputs. Each official release gets one
fixture. The candidate fixture must exist before its release.

The fixtures contain synthetic Profile state, provider settings, update state,
Sanitized Desktop State, and retained provider usage. The usage includes
complete and partial days with synthetic pricing evidence. It uses opaque
fixture keys instead of user paths. The fixtures do not contain credentials,
sessions, signing data, or recovery data. Historical fixtures contain only the
tables and fields that their release knew.

Run this command to create or replace the sole candidate fixture:

```sh
bun scripts/generate-database-fixtures.ts
```

The generator does not delete or write an official fixture database. When a
candidate becomes a published Release, change its definition to `official`,
set its exact tag commit, and add one new candidate definition. The generator
keeps the promoted database bytes and creates only the new candidate.

Run this command to check the stored hashes, SQLite integrity, foreign keys,
privacy markers, and sidecar absence:

```sh
bun scripts/generate-database-fixtures.ts --check
```

The check validates every official and candidate fixture and its stored hash.
It also compares each official `sourceCommit` with its local Git tag when that
tag is available.

Do not edit or delete an official fixture. Add a fixture for the next release
instead.
