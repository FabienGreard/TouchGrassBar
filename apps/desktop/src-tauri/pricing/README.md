# Bundled OpenAI pricing

`openai-standard.json` is the offline price source for API-equivalent cost.
The application embeds this file at compile time. It does not download price
updates.

To update prices, edit the manifest, change its `basis`, test the effective
date ranges, and release a new application version. On the first run of that
version, TouchGrassBar reprices the private SQLite model-day rows. It does not
rescan rollout files for a price-only change.

The index also stores a semantic fingerprint of the validated manifest. This
causes repricing if a price changes but the basis was not changed by mistake.
The basis remains the readable price-book version that the application can
show in sanitized output.

The Rust parser rejects unknown fields and invalid manifests. Keep the format
suitable for a future signed remote manifest, but do not add remote updates as
part of issue #21.
