# Bundled OpenAI pricing

`openai-standard.json` is the offline price source for API-equivalent cost.
The application embeds this file at compile time. It does not download price
updates.

To update prices, edit the manifest, change its `basis`, test the effective
date ranges, and release a new application version. On the first run of that
version, TouchGrassBar reprices the private SQLite model-day rows. It does not
rescan rollout files for a price-only change.

The index stores a semantic fingerprint of the validated manifest to detect an
update even if the basis was not changed by mistake. Each private model-day row
also stores the fingerprint of its applicable pricing rule. A manifest update
recalculates only rows whose applicable rule changed. The basis remains the
readable price-book version that the application can show in sanitized output.

The Rust parser rejects unknown fields and invalid manifests. Keep the format
suitable for a future signed remote manifest, but do not add remote updates as
part of issue #21.

## Unknown models

When the local debug report finds an unknown model, check the official OpenAI
API pricing page, model catalog, Codex rate card, and official Codex source.
Add a manifest entry only when those sources define every applicable input,
cached-input, cache-write, output, effective-date, and long-context rule. Add
an alias only when an official source defines it.

If any required price or alias is missing, leave the model out of the manifest.
That model's local tokens stay unpriced. A period can still show a modeled best
estimate when other priced local evidence supplies a defensible rate; its
coverage reports how much local detail was priced. If the period has no usable
priced evidence, API-equivalent cost stays unavailable while account Observed
Tokens remain visible. Updating the manifest and releasing the application are
manual operations.
