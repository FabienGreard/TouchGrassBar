import { Database } from "bun:sqlite";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { access, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

type ReleaseStatus = "official" | "candidate";

type UsageFact = {
  day: string;
  observedTokens: number;
  pricedTokens: number;
  costUsd: number;
  complete: boolean;
  pricingBasis: string;
  pricingFingerprint: string;
};

type FixtureDefinition = {
  tag: string;
  sourceCommit: string;
  releaseStatus: ReleaseStatus;
  revision: string;
  lifecycleVersion: 4 | 5;
  updateStateVersion: 1 | 2 | 3;
  codexUsageIndexVersion: 2 | 6 | 7 | 8 | 9;
  hasClaudeUsageIndex: boolean;
  hasTopModelUsage: boolean;
  hasExplicitVersions: boolean;
};

type FixtureManifestEntry = {
  tag: string;
  database: string;
  sha256: string;
  sourceCommit: string;
  releaseStatus: ReleaseStatus;
  sourceSchema: {
    databaseFormat: number;
    lifecycle: number;
    sanitizedDesktopState: 4 | 5 | 6 | 7;
    codexUsageIndex: 2 | 3 | 6 | 7 | 8 | 9;
    claudeUsageIndex: 3 | 4 | 7 | null;
    updateState: 1 | 2 | 3;
    databaseCoordinator: 1 | null;
  };
  sourceFeatures: {
    providerSettings: boolean;
    automaticUpdateChecks: boolean;
    claudeUsageIndex: boolean;
    retainedUsage: boolean;
    topModelUsage: boolean;
  };
  expectedState: {
    revision: string;
    profile: {
      status: "ready";
      displayName: string;
      touchGrassId: string;
    };
    providerSettingsAfterUpgrade: {
      codex: boolean;
      claude: boolean;
    };
    usage: {
      codex: UsageFact[];
      claude: UsageFact[];
    };
    automaticChecksEnabledAfterUpgrade: boolean;
    lastAutomaticCheckAt: number;
    offeredVersion: string;
    minimumRequiredVersion: string;
  };
};

type FixtureManifest = {
  formatVersion: number;
  generatedBy?: string;
  fixtures: FixtureManifestEntry[];
};

const fixturesRoot = join(
  import.meta.dir,
  "..",
  "apps",
  "desktop",
  "src-tauri",
  "tests",
  "fixtures",
  "releases",
);
const manifestPath = join(fixturesRoot, "manifest.json");
const databaseName = "touchgrassbar.sqlite3";
const profileDisplayName = "Fixture Tokenmaxxer";
const generatedAt = "2026-01-02T03:04:05Z";

const definitions: FixtureDefinition[] = [
  {
    tag: "v0.0.3",
    sourceCommit: "1cb6d7a8eb308d297753f1a97cf1be449a6de9ac",
    releaseStatus: "official",
    revision: "303",
    lifecycleVersion: 4,
    updateStateVersion: 1,
    codexUsageIndexVersion: 2,
    hasClaudeUsageIndex: false,
    hasTopModelUsage: false,
    hasExplicitVersions: false,
  },
  {
    tag: "v0.0.4",
    sourceCommit: "eb809d7ba737c37f0c007c318a653c098b3512c6",
    releaseStatus: "official",
    revision: "304",
    lifecycleVersion: 4,
    updateStateVersion: 1,
    codexUsageIndexVersion: 2,
    hasClaudeUsageIndex: false,
    hasTopModelUsage: false,
    hasExplicitVersions: false,
  },
  {
    tag: "v0.0.5",
    sourceCommit: "7cae76253f7f5b9f318f8dcc04c90482d3816db0",
    releaseStatus: "official",
    revision: "305",
    lifecycleVersion: 4,
    updateStateVersion: 1,
    codexUsageIndexVersion: 2,
    hasClaudeUsageIndex: false,
    hasTopModelUsage: false,
    hasExplicitVersions: false,
  },
  {
    tag: "v0.0.6",
    sourceCommit: "2c6145ec9381eb188ca4d4921a2f9a2d66db626b",
    releaseStatus: "official",
    revision: "306",
    lifecycleVersion: 4,
    updateStateVersion: 2,
    codexUsageIndexVersion: 2,
    hasClaudeUsageIndex: false,
    hasTopModelUsage: false,
    hasExplicitVersions: false,
  },
  {
    tag: "v0.0.7",
    sourceCommit: "815dece502f2d762c7327d4a2e19fb007bcd665e",
    releaseStatus: "official",
    revision: "307",
    lifecycleVersion: 5,
    updateStateVersion: 2,
    codexUsageIndexVersion: 2,
    hasClaudeUsageIndex: true,
    hasTopModelUsage: true,
    hasExplicitVersions: false,
  },
  {
    tag: "v0.0.8",
    sourceCommit: "78833ce632150750c40f483d2ffde015de35a65e",
    releaseStatus: "official",
    revision: "308",
    lifecycleVersion: 5,
    updateStateVersion: 2,
    codexUsageIndexVersion: 2,
    hasClaudeUsageIndex: true,
    hasTopModelUsage: true,
    hasExplicitVersions: false,
  },
  {
    tag: "v0.0.9",
    sourceCommit: "d01e60e067dc1202f45908851fc271ac78b5e5df",
    releaseStatus: "official",
    revision: "309",
    lifecycleVersion: 5,
    updateStateVersion: 3,
    codexUsageIndexVersion: 6,
    hasClaudeUsageIndex: true,
    hasTopModelUsage: true,
    hasExplicitVersions: true,
  },
  {
    tag: "v0.0.10",
    sourceCommit: "6b8a7e0d0ad24d67918a0cd711062197253c927a",
    releaseStatus: "official",
    revision: "310",
    lifecycleVersion: 5,
    updateStateVersion: 3,
    codexUsageIndexVersion: 8,
    hasClaudeUsageIndex: true,
    hasTopModelUsage: true,
    hasExplicitVersions: true,
  },
  {
    tag: "v0.0.11",
    sourceCommit: "fd573fc6f1f1c793fb57eb4ef3e45144c06f1d9f",
    releaseStatus: "official",
    revision: "311",
    lifecycleVersion: 5,
    updateStateVersion: 3,
    codexUsageIndexVersion: 8,
    hasClaudeUsageIndex: true,
    hasTopModelUsage: true,
    hasExplicitVersions: true,
  },
  {
    tag: "v0.0.12",
    sourceCommit: "696c12c8be762f3714e7d91b900f19544d2fcd09",
    releaseStatus: "official",
    revision: "312",
    lifecycleVersion: 5,
    updateStateVersion: 3,
    codexUsageIndexVersion: 8,
    hasClaudeUsageIndex: true,
    hasTopModelUsage: true,
    hasExplicitVersions: true,
  },
  {
    tag: "v0.0.13",
    sourceCommit: "candidate",
    releaseStatus: "candidate",
    revision: "313",
    lifecycleVersion: 5,
    updateStateVersion: 3,
    codexUsageIndexVersion: 9,
    hasClaudeUsageIndex: true,
    hasTopModelUsage: true,
    hasExplicitVersions: true,
  },
];

function readModelVersion(definition: FixtureDefinition): 4 | 6 | 7 {
  if (!definition.hasExplicitVersions) {
    return 4;
  }
  return definition.codexUsageIndexVersion >= 7 ? 7 : 6;
}

function readModelContractVersion(definition: FixtureDefinition): 3 | 4 {
  return definition.hasExplicitVersions ? 4 : 3;
}

function codexUsageVersion(definition: FixtureDefinition): 2 | 6 | 7 | 8 | 9 {
  return definition.codexUsageIndexVersion;
}

function claudeUsageVersion(definition: FixtureDefinition): 3 | 7 | null {
  if (!definition.hasClaudeUsageIndex) {
    return null;
  }
  return definition.hasExplicitVersions ? 7 : 3;
}

const unavailablePeriods = {
  scanStatus: "unavailable",
  todayScanStatus: "unavailable",
  sevenDayScanStatus: "unavailable",
  thirtyDayScanStatus: "unavailable",
  today: { availability: "unavailable" },
  sevenDays: { availability: "unavailable" },
  thirtyDays: { availability: "unavailable" },
};

function touchGrassId(definition: FixtureDefinition): string {
  return `fixture-public-${definition.tag.slice(1).replaceAll(".", "-")}`;
}

function updateTimestamp(definition: FixtureDefinition): number {
  return 1_700_000_000 + Number.parseInt(definition.revision, 10);
}

function usageOffset(definition: FixtureDefinition): number {
  return Number.parseInt(definition.revision, 10) - 300;
}

function codexUsageFacts(definition: FixtureDefinition): UsageFact[] {
  const offset = usageOffset(definition);
  return [
    {
      day: "2026-01-01",
      observedTokens: 1_000 + offset,
      pricedTokens: 800 + offset,
      costUsd: 1.25,
      complete: true,
      pricingBasis: "synthetic-codex-catalog-v1",
      pricingFingerprint: "synthetic-codex-price-v1",
    },
    {
      day: "2026-01-02",
      observedTokens: 1_200 + offset,
      pricedTokens: 900 + offset,
      costUsd: 1.5,
      complete: false,
      pricingBasis: "synthetic-codex-catalog-v1",
      pricingFingerprint: "synthetic-codex-price-v1",
    },
  ];
}

function claudeUsageFacts(definition: FixtureDefinition): UsageFact[] {
  if (!definition.hasClaudeUsageIndex) return [];
  const offset = usageOffset(definition);
  return [
    {
      day: "2026-01-01",
      observedTokens: 700 + offset,
      pricedTokens: 600 + offset,
      costUsd: 0.75,
      complete: true,
      pricingBasis: "synthetic-claude-catalog-v1",
      pricingFingerprint: "synthetic-claude-price-v1",
    },
    {
      day: "2026-01-02",
      observedTokens: 900 + offset,
      pricedTokens: 700 + offset,
      costUsd: 1,
      complete: false,
      pricingBasis: "synthetic-claude-catalog-v1",
      pricingFingerprint: "synthetic-claude-price-v1",
    },
  ];
}

function providerSettingsAfterUpgrade(definition: FixtureDefinition) {
  return {
    codex: definition.lifecycleVersion < 5,
    claude: true,
  };
}

function snapshot(definition: FixtureDefinition): Record<string, unknown> {
  const codexFacts = codexUsageFacts(definition);
  const codexToday = codexFacts[1];
  const codexTotal = codexFacts.reduce(
    (total, fact) => ({
      tokens: total.tokens + fact.observedTokens,
      cost: total.cost + fact.costUsd,
    }),
    { tokens: 0, cost: 0 },
  );
  const availableTotal = (
    availability: "current" | "stale",
    coverage: "complete" | "partial",
    observedTokens: number,
    costUsd: number,
  ) => ({
    availability,
    evidenceBasis: "locally-derived",
    coverage,
    observedAt: generatedAt,
    observedTokens,
    apiEquivalentCostUsd: costUsd,
    trendPercent: null,
    trendPreviousTokens: null,
    apiEquivalentCostBasis: "synthetic-codex-catalog-v1",
    apiEquivalentCostQuality: "local-only",
  });
  const codexUsage = {
    scanStatus: "complete",
    todayScanStatus: "complete",
    sevenDayScanStatus: "complete",
    thirtyDayScanStatus: "complete",
    today: availableTotal(
      "current",
      codexToday.complete ? "complete" : "partial",
      codexToday.observedTokens,
      codexToday.costUsd,
    ),
    sevenDays: availableTotal("current", "partial", codexTotal.tokens, codexTotal.cost),
    thirtyDays: availableTotal("stale", "partial", codexTotal.tokens, codexTotal.cost),
  };
  const topModelUsage = definition.hasTopModelUsage
    ? { model: "GPT 5.2", observedTokens: codexToday.observedTokens }
    : undefined;
  const provider = (
    name: "codex" | "claude",
    displayName: string,
    usage: Record<string, unknown>,
  ) => {
    const presentation: Record<string, unknown> = {
      provider: name,
      displayName,
      presence: name === "codex" ? "detected" : "unavailable",
      quota: {
        availability: "unavailable",
        provider: name,
        quotaLanes: [],
      },
      usage,
    };
    if (definition.hasTopModelUsage) {
      presentation.topModelUsage = name === "codex" ? topModelUsage : null;
    }
    return presentation;
  };
  const state: Record<string, unknown> = {
    contractVersion: readModelContractVersion(definition),
    generatedAt,
    revision: definition.revision,
    providers: [
      provider("codex", "Codex", codexUsage),
      provider("claude", "Claude", unavailablePeriods),
    ],
    combinedUsage: codexUsage,
    sync: {
      status: "unavailable",
      lastSuccessfulAt: null,
    },
    profile: {
      status: "ready",
      displayName: profileDisplayName,
      touchGrassId: touchGrassId(definition),
    },
  };
  if (definition.hasTopModelUsage) {
    state.topModelUsage = topModelUsage;
  }
  return state;
}

function createLifecycleSchema(database: Database, definition: FixtureDefinition): void {
  database.exec(`
    CREATE TABLE lifecycle_state (
      singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
      bootstrap_completed INTEGER NOT NULL CHECK (bootstrap_completed IN (0, 1)),
      profile_provisioning TEXT NOT NULL CHECK (
        profile_provisioning IN ('not-authorized', 'profile-pending', 'ready')
      ),
      public_participation_authorized INTEGER NOT NULL CHECK (
        public_participation_authorized IN (0, 1)
      ),
      profile_retry_pending INTEGER NOT NULL CHECK (profile_retry_pending IN (0, 1)),
      backfill_window_days INTEGER CHECK (backfill_window_days = 30),
      display_name TEXT CHECK (
        display_name IS NULL OR (length(trim(display_name)) BETWEEN 1 AND 40)
      ),
      touch_grass_id TEXT,
      recovery_disclosure_pending INTEGER NOT NULL DEFAULT 0 CHECK (
        recovery_disclosure_pending IN (0, 1)
      )
    );
  `);
  database
    .query(
      `INSERT INTO lifecycle_state (
         singleton,
         bootstrap_completed,
         profile_provisioning,
         public_participation_authorized,
         profile_retry_pending,
         backfill_window_days,
         display_name,
         touch_grass_id,
         recovery_disclosure_pending
       ) VALUES (1, 1, 'ready', 1, 0, 30, ?1, ?2, 0)`,
    )
    .run(profileDisplayName, touchGrassId(definition));
  if (definition.lifecycleVersion === 5) {
    database.exec(`
      CREATE TABLE provider_settings (
        provider TEXT PRIMARY KEY CHECK (provider IN ('codex', 'claude')),
        enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
      );
      INSERT INTO provider_settings (provider, enabled)
      VALUES ('codex', 0), ('claude', 1);
    `);
  }
  database.exec(`PRAGMA user_version = ${definition.lifecycleVersion}`);
}

function createReadModelSchema(database: Database, definition: FixtureDefinition): void {
  const schemaVersion = readModelVersion(definition);
  const contractVersion = readModelContractVersion(definition);
  database.exec(`
    CREATE TABLE sanitized_desktop_state (
      singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
      schema_version INTEGER NOT NULL CHECK (schema_version = ${schemaVersion}),
      contract_version INTEGER NOT NULL CHECK (contract_version = ${contractVersion}),
      revision TEXT NOT NULL CHECK (
        length(revision) > 0 AND revision NOT GLOB '*[^0-9]*'
      ),
      snapshot_json TEXT NOT NULL
    );
  `);
  database
    .query(
      `INSERT INTO sanitized_desktop_state (
         singleton, schema_version, contract_version, revision, snapshot_json
       ) VALUES (1, ?1, ?2, ?3, ?4)`,
    )
    .run(schemaVersion, contractVersion, definition.revision, JSON.stringify(snapshot(definition)));
  setModuleVersion(database, "sanitized-desktop-state", schemaVersion);
}

function createCodexUsageSchema(database: Database, definition: FixtureDefinition): void {
  const accountMetaTimestampColumn =
    definition.codexUsageIndexVersion >= 8 ? "refreshed_at" : "observed_at";
  const accountDayTimestampColumn =
    definition.codexUsageIndexVersion >= 8 ? ",\n      observed_at TEXT NOT NULL" : "";
  const activeTurnColumn = definition.hasExplicitVersions ? ",\n      active_turn_id TEXT" : "";
  const versionNineFileColumns =
    definition.codexUsageIndexVersion >= 9
      ? `,
      task_counter_reset_pending INTEGER NOT NULL DEFAULT 0,
      provider_ordinal_mode TEXT NOT NULL DEFAULT 'unknown'`
      : "";
  const currentFileColumns = definition.hasExplicitVersions
    ? `,
      lineage_mode TEXT NOT NULL DEFAULT 'unknown',
      leaf_session_id TEXT,
      parent_session_id TEXT,
      parent_identity_explicit INTEGER NOT NULL DEFAULT 0,
      fork_timestamp_ns INTEGER,
      embedded_ancestor_seen INTEGER NOT NULL DEFAULT 0,
      lineage_invalid INTEGER NOT NULL DEFAULT 0,
      parent_dependency_key TEXT,
      parent_baseline_input INTEGER,
      parent_baseline_cached_input INTEGER,
      parent_baseline_cache_write_input INTEGER,
      parent_baseline_output INTEGER,
      parent_baseline_reasoning_output INTEGER,
      parent_baseline_total INTEGER,
      last_turn_context_is_first INTEGER NOT NULL DEFAULT 0,
      last_turn_context_ordinal INTEGER,
      marker_based_boundary INTEGER NOT NULL DEFAULT 0,
      marker_candidate_invalidated INTEGER NOT NULL DEFAULT 0,
      marker_local_confirmation INTEGER,
      accounting_ready INTEGER NOT NULL DEFAULT 0,
      parser_error_seen INTEGER NOT NULL DEFAULT 0,
      snapshot_last_timestamp_ns INTEGER,
      snapshot_timestamp_regressed INTEGER NOT NULL DEFAULT 0${versionNineFileColumns}`
    : "";
  const pricingModeColumn = definition.hasExplicitVersions
    ? `,
      pricing_mode TEXT NOT NULL CHECK(pricing_mode IN ('standard', 'fast'))`
    : "";
  const fileTurnDayColumn =
    definition.codexUsageIndexVersion >= 7 ? ",\n        day TEXT NOT NULL" : "";
  const fileTurnPrimaryKey =
    definition.codexUsageIndexVersion >= 7
      ? "PRIMARY KEY (path, turn_id, day)"
      : "PRIMARY KEY (path, turn_id)";
  const currentCodexObjects = definition.hasExplicitVersions
    ? `
      CREATE TABLE codex_usage_file_turns (
        path TEXT NOT NULL,
        turn_id TEXT NOT NULL${fileTurnDayColumn},
        ${fileTurnPrimaryKey},
        FOREIGN KEY(path) REFERENCES codex_usage_files(path) ON DELETE CASCADE
      );
      CREATE TABLE codex_usage_fast_turns (
        turn_id TEXT PRIMARY KEY NOT NULL,
        model TEXT
      );
      CREATE TABLE codex_usage_token_snapshots (
        path TEXT NOT NULL,
        record_ordinal INTEGER NOT NULL,
        timestamp_ns INTEGER NOT NULL,
        input_tokens INTEGER NOT NULL,
        cached_input_tokens INTEGER NOT NULL,
        cache_write_input_tokens INTEGER NOT NULL,
        output_tokens INTEGER NOT NULL,
        reasoning_output_tokens INTEGER NOT NULL,
        total_tokens INTEGER NOT NULL,
        PRIMARY KEY (path, record_ordinal),
        FOREIGN KEY(path) REFERENCES codex_usage_files(path) ON DELETE CASCADE
      );
      CREATE INDEX codex_usage_file_turns_by_turn_id
        ON codex_usage_file_turns(turn_id);
      CREATE INDEX codex_usage_files_by_leaf_session
        ON codex_usage_files(leaf_session_id);
      CREATE INDEX codex_usage_snapshots_by_path_timestamp
        ON codex_usage_token_snapshots(path, timestamp_ns, record_ordinal);
    `
    : "";
  database.exec(`
    CREATE TABLE codex_usage_index_meta (
      key TEXT PRIMARY KEY NOT NULL,
      value TEXT NOT NULL
    );
    CREATE TABLE codex_account_usage_meta (
      singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
      ${accountMetaTimestampColumn} TEXT NOT NULL
    );
    CREATE TABLE codex_account_usage_days (
      day TEXT PRIMARY KEY NOT NULL,
      tokens INTEGER NOT NULL${accountDayTimestampColumn}
    );
    CREATE TABLE codex_usage_files (
      path TEXT PRIMARY KEY NOT NULL,
      file_identity TEXT NOT NULL,
      size_bytes INTEGER NOT NULL,
      modified_ns INTEGER NOT NULL,
      parsed_offset INTEGER NOT NULL,
      parsed_prefix_anchor TEXT,
      parser_version INTEGER NOT NULL,
      completion_state TEXT NOT NULL,
      deferred_until_day TEXT,
      active_model TEXT${activeTurnColumn},
      baseline_is_inherited INTEGER,
      history_start_ordinal INTEGER,
      record_ordinal INTEGER NOT NULL DEFAULT 0,
      usage_excluded INTEGER NOT NULL DEFAULT 0,
      schema_supported INTEGER NOT NULL,
      previous_input INTEGER,
      previous_cached_input INTEGER,
      previous_cache_write_input INTEGER,
      previous_output INTEGER,
      previous_reasoning_output INTEGER,
      previous_total INTEGER${currentFileColumns}
    );
    CREATE TABLE codex_usage_file_model_days (
      path TEXT NOT NULL,
      day TEXT NOT NULL,
      model TEXT NOT NULL,
      pricing_input_tokens INTEGER NOT NULL${pricingModeColumn},
      input_tokens INTEGER NOT NULL,
      cached_input_tokens INTEGER NOT NULL,
      cache_write_input_tokens INTEGER NOT NULL,
      output_tokens INTEGER NOT NULL,
      reasoning_output_tokens INTEGER NOT NULL,
      observed_tokens INTEGER NOT NULL,
      cost_usd REAL,
      pricing_basis TEXT,
      pricing_fingerprint TEXT,
      complete INTEGER NOT NULL,
      observed_through TEXT NOT NULL,
      PRIMARY KEY (
        path, day, model, pricing_input_tokens${
          definition.hasExplicitVersions ? ", pricing_mode" : ""
        }
      ),
      FOREIGN KEY(path) REFERENCES codex_usage_files(path) ON DELETE CASCADE
    );
    CREATE TABLE codex_usage_file_days (
      path TEXT NOT NULL,
      day TEXT NOT NULL,
      observed_tokens INTEGER NOT NULL,
      priced_tokens INTEGER NOT NULL,
      cost_usd REAL NOT NULL,
      complete INTEGER NOT NULL,
      observed_through TEXT NOT NULL,
      priced_observed_through TEXT,
      pricing_fingerprint TEXT,
      PRIMARY KEY (path, day),
      FOREIGN KEY(path) REFERENCES codex_usage_files(path) ON DELETE CASCADE
    );
    CREATE INDEX codex_usage_model_days_by_day
      ON codex_usage_file_model_days(day);
    CREATE INDEX codex_usage_unpriced_model_days
      ON codex_usage_file_model_days(day, model, cache_write_input_tokens)
      WHERE cost_usd IS NULL;
    ${currentCodexObjects}
  `);
  setModuleVersion(database, "codex-usage-index", codexUsageVersion(definition));

  const path = `fixture-codex-session-${definition.tag}`;
  const currentFileInsertColumns = definition.hasExplicitVersions
    ? ", lineage_mode, accounting_ready"
    : "";
  const currentFileInsertValues = definition.hasExplicitVersions ? ", 'root', 1" : "";
  database
    .query(
      `INSERT INTO codex_usage_files (
         path, file_identity, size_bytes, modified_ns, parsed_offset,
         parsed_prefix_anchor, parser_version, completion_state, schema_supported
         ${currentFileInsertColumns}
       ) VALUES (
         ?1, ?2, 4096, 1700000000, 4096, ?3, 8, 'complete', 1
         ${currentFileInsertValues}
       )`,
    )
    .run(
      path,
      `fixture-codex-identity-${definition.tag}`,
      `fixture-codex-anchor-${definition.tag}`,
    );
  database
    .query(
      `INSERT INTO codex_account_usage_meta(singleton, ${accountMetaTimestampColumn})
       VALUES (1, ?1)`,
    )
    .run(generatedAt);
  const modelDay = database.query(
    `INSERT INTO codex_usage_file_model_days (
       path, day, model, pricing_input_tokens${
         definition.hasExplicitVersions ? ", pricing_mode" : ""
       }, input_tokens,
       cached_input_tokens, cache_write_input_tokens, output_tokens,
       reasoning_output_tokens, observed_tokens, cost_usd, pricing_basis,
       pricing_fingerprint, complete, observed_through
     ) VALUES (
       ?1, ?2, 'gpt-5.2', ?3${
         definition.hasExplicitVersions ? ", 'standard'" : ""
       }, ?4, 100, 50, 100, 50, ?5, ?6, ?7, ?8, ?9, ?10
     )`,
  );
  const fileDay = database.query(
    `INSERT INTO codex_usage_file_days (
       path, day, observed_tokens, priced_tokens, cost_usd, complete,
       observed_through, priced_observed_through, pricing_fingerprint
     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)`,
  );
  const accountDay = database.query(
    definition.codexUsageIndexVersion >= 8
      ? `INSERT INTO codex_account_usage_days(day, tokens, observed_at)
         VALUES (?1, ?2, ?3)`
      : `INSERT INTO codex_account_usage_days(day, tokens) VALUES (?1, ?2)`,
  );
  for (const fact of codexUsageFacts(definition)) {
    modelDay.run(
      path,
      fact.day,
      fact.pricedTokens,
      fact.observedTokens - 300,
      fact.observedTokens,
      fact.costUsd,
      fact.pricingBasis,
      fact.pricingFingerprint,
      Number(fact.complete),
      `${fact.day}T23:59:59Z`,
    );
    fileDay.run(
      path,
      fact.day,
      fact.observedTokens,
      fact.pricedTokens,
      fact.costUsd,
      Number(fact.complete),
      `${fact.day}T23:59:59Z`,
      fact.pricingFingerprint,
    );
    if (definition.codexUsageIndexVersion >= 8) {
      accountDay.run(fact.day, fact.observedTokens + 100, generatedAt);
    } else {
      accountDay.run(fact.day, fact.observedTokens + 100);
    }
  }
}

function createClaudeUsageSchema(database: Database, definition: FixtureDefinition): void {
  const aggregateAppliedColumn = definition.hasExplicitVersions
    ? `,
      aggregate_applied INTEGER NOT NULL DEFAULT 1
        CHECK (aggregate_applied IN (0, 1))`
    : "";
  const modeledCostColumn = definition.hasExplicitVersions
    ? `,
      cost_modeled INTEGER NOT NULL DEFAULT 0
        CHECK (cost_modeled IN (0, 1))`
    : "";
  const correctionColumns = definition.hasExplicitVersions
    ? `,
      correction_provenance TEXT,
      correction_source_revision INTEGER,
      CHECK (
        (
          correction_provenance IS NULL
          AND correction_source_revision IS NULL
        ) OR (
          correction_provenance = 'parser-correction'
          AND correction_source_revision >= 1
          AND correction_source_revision <= revision
        )
      )`
    : "";
  database.exec(`
    CREATE TABLE claude_usage_index_meta (
      key TEXT PRIMARY KEY NOT NULL,
      value TEXT NOT NULL
    );
    CREATE TABLE claude_usage_files (
      path TEXT PRIMARY KEY NOT NULL,
      file_identity TEXT NOT NULL,
      size_bytes INTEGER NOT NULL,
      modified_ns INTEGER NOT NULL,
      parsed_offset INTEGER NOT NULL,
      resume_anchor TEXT,
      parser_version INTEGER NOT NULL,
      completion_state TEXT NOT NULL
    );
    CREATE TABLE claude_usage_messages (
      frame_key TEXT PRIMARY KEY NOT NULL,
      supersedes_frame_key TEXT,
      message_key TEXT NOT NULL,
      day TEXT NOT NULL,
      observed_at TEXT NOT NULL,
      model TEXT NOT NULL,
      input_tokens INTEGER NOT NULL,
      cache_creation_input_tokens INTEGER NOT NULL,
      cache_read_input_tokens INTEGER NOT NULL,
      output_tokens INTEGER NOT NULL,
      cache_creation_5m_input_tokens INTEGER,
      cache_creation_1h_input_tokens INTEGER,
      service_tier TEXT,
      inference_geo TEXT,
      speed TEXT,
      web_search_requests INTEGER,
      web_fetch_requests INTEGER,
      code_execution_requests INTEGER,
      has_unknown_paid_server_tool INTEGER NOT NULL,
      observed_tokens INTEGER NOT NULL,
      complete INTEGER NOT NULL,
      parser_version INTEGER NOT NULL
    );
    CREATE INDEX claude_usage_messages_by_day
      ON claude_usage_messages(day);
    CREATE INDEX claude_usage_messages_by_message
      ON claude_usage_messages(message_key);
    CREATE INDEX claude_usage_messages_by_superseded_frame
      ON claude_usage_messages(supersedes_frame_key);
    CREATE TABLE claude_usage_frames (
      frame_key TEXT PRIMARY KEY NOT NULL,
      day TEXT NOT NULL,
      observed_at TEXT NOT NULL,
      parser_version INTEGER NOT NULL
    );
    CREATE INDEX claude_usage_frames_by_day
      ON claude_usage_frames(day);
    CREATE TABLE claude_usage_message_supersedes (
      replacement_frame_key TEXT NOT NULL,
      superseded_frame_key TEXT NOT NULL,
      parser_version INTEGER NOT NULL${aggregateAppliedColumn},
      PRIMARY KEY(replacement_frame_key, superseded_frame_key),
      FOREIGN KEY(replacement_frame_key)
        REFERENCES claude_usage_frames(frame_key) ON DELETE CASCADE
    );
    CREATE INDEX claude_usage_supersedes_by_superseded_frame
      ON claude_usage_message_supersedes(superseded_frame_key, parser_version);
    CREATE TABLE claude_usage_daily (
      day TEXT PRIMARY KEY NOT NULL,
      observed_tokens INTEGER NOT NULL,
      coverage TEXT NOT NULL CHECK (coverage IN ('complete', 'partial')),
      observed_through TEXT NOT NULL,
      revision INTEGER NOT NULL CHECK (revision >= 1),
      priced_tokens INTEGER NOT NULL DEFAULT 0,
      cost_usd REAL${modeledCostColumn},
      pricing_basis TEXT,
      pricing_fingerprint TEXT${correctionColumns}
    );
  `);
  const schemaVersion = claudeUsageVersion(definition);
  if (schemaVersion === null) {
    throw new Error("Claude usage schema is not available for this fixture.");
  }
  setModuleVersion(database, "claude-usage-index", schemaVersion);

  const path = `fixture-claude-session-${definition.tag}`;
  const facts = claudeUsageFacts(definition);
  database
    .query(
      `INSERT INTO claude_usage_index_meta(key, value)
       VALUES ('usage_aggregate_parser_version', '4')`,
    )
    .run();
  database
    .query(
      `INSERT INTO claude_usage_files (
         path, file_identity, size_bytes, modified_ns, parsed_offset,
         resume_anchor, parser_version, completion_state
       ) VALUES (?1, ?2, 4096, 1700000000, 4096, ?3, 4, 'complete')`,
    )
    .run(
      path,
      `fixture-claude-identity-${definition.tag}`,
      `fixture-claude-anchor-${definition.tag}`,
    );
  const frame = database.query(
    `INSERT INTO claude_usage_frames(frame_key, day, observed_at, parser_version)
     VALUES (?1, ?2, ?3, 4)`,
  );
  const message = database.query(
    `INSERT INTO claude_usage_messages (
       frame_key, supersedes_frame_key, message_key, day, observed_at, model,
       input_tokens, cache_creation_input_tokens, cache_read_input_tokens,
       output_tokens, cache_creation_5m_input_tokens,
       cache_creation_1h_input_tokens, service_tier, inference_geo, speed,
       web_search_requests, web_fetch_requests, code_execution_requests,
       has_unknown_paid_server_tool, observed_tokens, complete, parser_version
     ) VALUES (
       ?1, ?2, ?3, ?4, ?5, 'claude-sonnet-4', ?6, 100, 50, 100,
       50, 50, 'standard', 'global', 'standard', 0, 0, 0, 0, ?7, ?8, 4
     )`,
  );
  const daily = database.query(
    `INSERT INTO claude_usage_daily (
       day, observed_tokens, coverage, observed_through, revision,
       priced_tokens, cost_usd${
         definition.hasExplicitVersions ? ", cost_modeled" : ""
       }, pricing_basis, pricing_fingerprint
     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7${definition.hasExplicitVersions ? ", 0" : ""}, ?8, ?9)`,
  );
  for (const [index, fact] of facts.entries()) {
    const frameKey = `fixture-claude-frame-${definition.tag}-${index + 1}`;
    const previousFrame = index === 0 ? null : `fixture-claude-frame-${definition.tag}-${index}`;
    frame.run(frameKey, fact.day, `${fact.day}T23:59:59Z`);
    message.run(
      frameKey,
      previousFrame,
      `fixture-claude-message-${definition.tag}`,
      fact.day,
      `${fact.day}T23:59:59Z`,
      fact.observedTokens - 250,
      fact.observedTokens,
      Number(fact.complete),
    );
    daily.run(
      fact.day,
      fact.observedTokens,
      fact.complete ? "complete" : "partial",
      `${fact.day}T23:59:59Z`,
      index + 1,
      fact.pricedTokens,
      fact.costUsd,
      fact.pricingBasis,
      fact.pricingFingerprint,
    );
  }
  database
    .query(
      `INSERT INTO claude_usage_message_supersedes (
         replacement_frame_key, superseded_frame_key, parser_version${
           definition.hasExplicitVersions ? ", aggregate_applied" : ""
         }
       ) VALUES (?1, ?2, 4${definition.hasExplicitVersions ? ", 1" : ""})`,
    )
    .run(`fixture-claude-frame-${definition.tag}-2`, `fixture-claude-frame-${definition.tag}-1`);
}

function createUsageSyncSchema(database: Database, definition: FixtureDefinition): void {
  const profileCompletionColumn =
    readModelVersion(definition) >= 7
      ? `,
      profile_backfill_completed INTEGER NOT NULL DEFAULT 0 CHECK(
        profile_backfill_completed IN (0, 1)
        AND (profile_backfill_completed = 0 OR active_generation = 1)
      )`
      : "";
  database.exec(`
    CREATE TABLE usage_sync_generations (
      active_generation INTEGER PRIMARY KEY,
      queue_state TEXT NOT NULL
        CHECK(queue_state IN ('active', 'blocked', 'abandoned')),
      CHECK(active_generation >= 1 AND active_generation <= 9007199254740991)
    ) STRICT;
    CREATE TABLE usage_sync_daily_aggregates (
      active_generation INTEGER NOT NULL,
      provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
      ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
      revision INTEGER NOT NULL
        CHECK(revision >= 1 AND revision <= 9007199254740991),
      aggregate_json TEXT NOT NULL CHECK(length(aggregate_json) <= 4096),
      PRIMARY KEY(active_generation, provider, ranking_day),
      FOREIGN KEY(active_generation)
        REFERENCES usage_sync_generations(active_generation)
    ) STRICT;
    CREATE TABLE usage_sync_generation_baselines (
      active_generation INTEGER NOT NULL,
      provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
      ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
      aggregate_json TEXT NOT NULL CHECK(length(aggregate_json) <= 4096),
      PRIMARY KEY(active_generation, provider, ranking_day),
      FOREIGN KEY(active_generation)
        REFERENCES usage_sync_generations(active_generation)
    ) STRICT;
    CREATE TABLE usage_sync_generation_activations (
      active_generation INTEGER PRIMARY KEY,
      ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
      activated_at INTEGER NOT NULL
        CHECK(activated_at >= 0 AND activated_at <= 9007199254740991)${profileCompletionColumn},
      FOREIGN KEY(active_generation)
        REFERENCES usage_sync_generations(active_generation)
    ) STRICT;
    CREATE TABLE usage_sync_latest_outbox (
      active_generation INTEGER NOT NULL,
      provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
      ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
      revision INTEGER NOT NULL
        CHECK(revision >= 1 AND revision <= 9007199254740991),
      snapshot_json TEXT NOT NULL CHECK(length(snapshot_json) <= 4096),
      correction_reason TEXT CHECK(
        correction_reason IS NULL OR correction_reason IN (
          'provider-replacement', 'parser-correction'
        )
      ),
      correction_revision INTEGER,
      queue_state TEXT NOT NULL
        CHECK(queue_state IN ('active', 'blocked', 'abandoned')),
      CHECK(
        (correction_reason IS NULL AND correction_revision IS NULL)
        OR (
          correction_reason IS NOT NULL
          AND correction_revision IS NOT NULL
          AND correction_revision >= 1
          AND correction_revision <= revision
          AND correction_revision <= 9007199254740991
        )
      ),
      PRIMARY KEY(active_generation, provider, ranking_day),
      FOREIGN KEY(active_generation)
        REFERENCES usage_sync_generations(active_generation)
    ) STRICT;
    CREATE INDEX usage_sync_latest_outbox_pending
      ON usage_sync_latest_outbox(
        active_generation, queue_state, ranking_day, provider
      );
    CREATE TABLE usage_sync_transfer_day_carryovers (
      active_generation INTEGER NOT NULL,
      provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
      ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
      carryover_kind TEXT NOT NULL CHECK(carryover_kind IN (
        'delayed-installation-marker', 'pending-segment'
      )),
      PRIMARY KEY(active_generation, provider, ranking_day),
      FOREIGN KEY(active_generation, provider, ranking_day)
        REFERENCES usage_sync_latest_outbox(
          active_generation, provider, ranking_day
        ) ON DELETE CASCADE
    ) STRICT;
    CREATE TABLE usage_sync_terminal_conflicts (
      active_generation INTEGER NOT NULL,
      provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
      ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
      revision INTEGER NOT NULL
        CHECK(revision >= 1 AND revision <= 9007199254740991),
      PRIMARY KEY(active_generation, provider, ranking_day, revision),
      FOREIGN KEY(active_generation)
        REFERENCES usage_sync_generations(active_generation)
    ) STRICT;
    CREATE TABLE usage_sync_provider_settings_outbox (
      active_generation INTEGER PRIMARY KEY,
      revision INTEGER NOT NULL
        CHECK(revision >= 1 AND revision <= 9007199254740991),
      codex_enabled INTEGER NOT NULL CHECK(codex_enabled IN (0, 1)),
      claude_enabled INTEGER NOT NULL CHECK(claude_enabled IN (0, 1)),
      delivery_state TEXT NOT NULL
        CHECK(delivery_state IN ('pending', 'synced', 'blocked', 'abandoned')),
      FOREIGN KEY(active_generation)
        REFERENCES usage_sync_generations(active_generation)
    ) STRICT;
    CREATE TABLE usage_sync_correction_lineage (
      provider TEXT NOT NULL CHECK(provider IN ('codex', 'claude')),
      ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
      source_revision INTEGER NOT NULL
        CHECK(source_revision >= 1 AND source_revision <= 9007199254740991),
      reason TEXT NOT NULL CHECK(reason = 'parser-correction'),
      consumed_generation INTEGER CHECK(
        consumed_generation IS NULL OR (
          consumed_generation >= 1
          AND consumed_generation <= 9007199254740991
        )
      ),
      PRIMARY KEY(provider, ranking_day)
    ) STRICT;
  `);
}

function createUpdateStateSchema(database: Database, definition: FixtureDefinition): void {
  const automaticChecksColumn =
    definition.updateStateVersion >= 2
      ? `automatic_checks_enabled INTEGER NOT NULL DEFAULT 1 CHECK (
           automatic_checks_enabled IN (0, 1)
         ),`
      : "";
  const storageTable =
    definition.updateStateVersion === 3
      ? "touchgrassbar_update_state_v3"
      : "touchgrassbar_update_state";
  database.exec(`
    CREATE TABLE ${storageTable} (
      singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
      ${automaticChecksColumn}
      last_automatic_check_at INTEGER,
      offered_version TEXT CHECK (
        offered_version IS NULL OR length(offered_version) BETWEEN 1 AND 64
      ),
      minimum_required_version TEXT CHECK (
        minimum_required_version IS NULL OR
        length(minimum_required_version) BETWEEN 1 AND 64
      )
    );
  `);
  const timestamp = updateTimestamp(definition);
  if (definition.updateStateVersion >= 2) {
    database
      .query(
        `INSERT INTO ${storageTable} (
           singleton,
           automatic_checks_enabled,
           last_automatic_check_at,
           offered_version,
           minimum_required_version
         ) VALUES (1, 0, ?1, '0.0.9', '0.0.3')`,
      )
      .run(timestamp);
  } else {
    database
      .query(
        `INSERT INTO ${storageTable} (
           singleton,
           last_automatic_check_at,
           offered_version,
           minimum_required_version
         ) VALUES (1, ?1, '0.0.9', '0.0.3')`,
      )
      .run(timestamp);
  }
  if (definition.updateStateVersion === 3) {
    database.exec(`
      CREATE VIEW touchgrassbar_update_state AS
        SELECT singleton, automatic_checks_enabled, last_automatic_check_at,
               offered_version, minimum_required_version
        FROM touchgrassbar_update_state_v3;
    `);
  }
}

function setModuleVersion(database: Database, module: string, version: number): void {
  database
    .query(
      `INSERT INTO touchgrassbar_schema_versions(module, version)
       VALUES (?1, ?2)`,
    )
    .run(module, version);
}

function scalarValue(row: unknown): unknown {
  if (row === null || typeof row !== "object") {
    return undefined;
  }
  return Object.values(row)[0];
}

function checkDatabase(databasePath: string): void {
  const database = new Database(databasePath, { readonly: true, strict: true });
  try {
    const integrity = scalarValue(database.query("PRAGMA integrity_check").get());
    if (integrity !== "ok") {
      throw new Error(`Fixture integrity check failed: ${databasePath}`);
    }
    if (database.query("PRAGMA foreign_key_check").all().length !== 0) {
      throw new Error(`Fixture foreign key check failed: ${databasePath}`);
    }
  } finally {
    database.close();
  }
}

function stringRows(database: Database, sql: string): string[] {
  return database
    .query(sql)
    .all()
    .map((row) => String(scalarValue(row)));
}

function storedUsageFacts(database: Database, provider: "codex" | "claude"): UsageFact[] {
  const rows =
    provider === "codex"
      ? database
          .query(
            `SELECT daily.day, daily.observed_tokens, daily.priced_tokens,
                    daily.cost_usd, daily.complete, model.pricing_basis,
                    daily.pricing_fingerprint
             FROM codex_usage_file_days AS daily
             JOIN codex_usage_file_model_days AS model
               ON model.path = daily.path AND model.day = daily.day
             ORDER BY daily.day`,
          )
          .all()
      : database
          .query(
            `SELECT day, observed_tokens, priced_tokens, cost_usd,
                    coverage = 'complete' AS complete, pricing_basis,
                    pricing_fingerprint
             FROM claude_usage_daily ORDER BY day`,
          )
          .all();
  return rows.map((row) => {
    const value = row as Record<string, unknown>;
    return {
      day: String(value.day),
      observedTokens: Number(value.observed_tokens),
      pricedTokens: Number(value.priced_tokens),
      costUsd: Number(value.cost_usd),
      complete: Boolean(value.complete),
      pricingBasis: String(value.pricing_basis),
      pricingFingerprint: String(value.pricing_fingerprint),
    };
  });
}

function validateFixtureContents(
  databasePath: string,
  definition: FixtureDefinition,
  entry: FixtureManifestEntry,
): void {
  if (entry.database !== `${definition.tag}/${databaseName}`) {
    throw new Error(`The database path is wrong for ${definition.tag}.`);
  }
  if (
    entry.sourceCommit !== definition.sourceCommit ||
    entry.releaseStatus !== definition.releaseStatus
  ) {
    throw new Error(`The source identity is wrong for ${definition.tag}.`);
  }
  const database = new Database(databasePath, { readonly: true, strict: true });
  try {
    const databaseFormat = Number(scalarValue(database.query("PRAGMA user_version").get()));
    const expectedDatabaseFormat = definition.hasExplicitVersions ? 7 : definition.lifecycleVersion;
    if (databaseFormat !== expectedDatabaseFormat) {
      throw new Error(`The database format is wrong for ${definition.tag}.`);
    }
    const tables = stringRows(
      database,
      `SELECT name FROM sqlite_schema
       WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
       ORDER BY name`,
    );
    const expectedTables = [
      "codex_account_usage_days",
      "codex_account_usage_meta",
      "codex_usage_file_days",
      "codex_usage_file_model_days",
      "codex_usage_files",
      "codex_usage_index_meta",
      "lifecycle_state",
      "sanitized_desktop_state",
      "touchgrassbar_schema_versions",
    ];
    expectedTables.push(
      definition.updateStateVersion === 3
        ? "touchgrassbar_update_state_v3"
        : "touchgrassbar_update_state",
    );
    if (definition.lifecycleVersion === 5) {
      expectedTables.push("provider_settings");
    }
    if (definition.hasClaudeUsageIndex) {
      expectedTables.push(
        "claude_usage_daily",
        "claude_usage_files",
        "claude_usage_frames",
        "claude_usage_index_meta",
        "claude_usage_message_supersedes",
        "claude_usage_messages",
      );
    }
    if (definition.hasExplicitVersions) {
      expectedTables.push(
        "codex_usage_fast_turns",
        "codex_usage_file_turns",
        "codex_usage_token_snapshots",
        "usage_sync_correction_lineage",
        "usage_sync_daily_aggregates",
        "usage_sync_generation_activations",
        "usage_sync_generation_baselines",
        "usage_sync_generations",
        "usage_sync_latest_outbox",
        "usage_sync_provider_settings_outbox",
        "usage_sync_terminal_conflicts",
        "usage_sync_transfer_day_carryovers",
      );
      const fileTurnColumns = stringRows(
        database,
        `SELECT name FROM pragma_table_info('codex_usage_file_turns')
         ORDER BY cid`,
      );
      const expectedFileTurnColumns =
        definition.codexUsageIndexVersion >= 7 ? ["path", "turn_id", "day"] : ["path", "turn_id"];
      if (JSON.stringify(fileTurnColumns) !== JSON.stringify(expectedFileTurnColumns)) {
        throw new Error(`The Codex file-turn shape is wrong for ${definition.tag}.`);
      }
      const accountDayColumns = stringRows(
        database,
        `SELECT name FROM pragma_table_info('codex_account_usage_days')
         ORDER BY cid`,
      );
      const expectedAccountDayColumns =
        definition.codexUsageIndexVersion >= 8
          ? ["day", "tokens", "observed_at"]
          : ["day", "tokens"];
      if (JSON.stringify(accountDayColumns) !== JSON.stringify(expectedAccountDayColumns)) {
        throw new Error(`The Codex account-day shape is wrong for ${definition.tag}.`);
      }
      const accountMetaColumns = stringRows(
        database,
        `SELECT name FROM pragma_table_info('codex_account_usage_meta')
         ORDER BY cid`,
      );
      const expectedAccountMetaColumns =
        definition.codexUsageIndexVersion >= 8
          ? ["singleton", "refreshed_at"]
          : ["singleton", "observed_at"];
      if (JSON.stringify(accountMetaColumns) !== JSON.stringify(expectedAccountMetaColumns)) {
        throw new Error(`The Codex account-meta shape is wrong for ${definition.tag}.`);
      }
      const activationColumns = stringRows(
        database,
        `SELECT name FROM pragma_table_info('usage_sync_generation_activations')
         ORDER BY cid`,
      );
      const expectedActivationColumns =
        readModelVersion(definition) >= 7
          ? ["active_generation", "ranking_day", "activated_at", "profile_backfill_completed"]
          : ["active_generation", "ranking_day", "activated_at"];
      if (JSON.stringify(activationColumns) !== JSON.stringify(expectedActivationColumns)) {
        throw new Error(`The usage activation shape is wrong for ${definition.tag}.`);
      }
    }
    expectedTables.sort();
    if (JSON.stringify(tables) !== JSON.stringify(expectedTables)) {
      throw new Error(`The table set is wrong for ${definition.tag}.`);
    }
    const views = stringRows(
      database,
      `SELECT name FROM sqlite_schema
       WHERE type = 'view' ORDER BY name`,
    );
    const expectedViews = definition.updateStateVersion === 3 ? ["touchgrassbar_update_state"] : [];
    if (JSON.stringify(views) !== JSON.stringify(expectedViews)) {
      throw new Error(`The view set is wrong for ${definition.tag}.`);
    }
    const triggers = stringRows(
      database,
      `SELECT name FROM sqlite_schema
       WHERE type = 'trigger' ORDER BY name`,
    );
    if (triggers.length !== 0) {
      throw new Error(`The update compatibility view is writable for ${definition.tag}.`);
    }

    const moduleVersions = database
      .query(
        `SELECT module, version FROM touchgrassbar_schema_versions
         ORDER BY module`,
      )
      .all() as { module: string; version: number }[];
    const expectedModuleVersions: { module: string; version: number }[] = [
      { module: "codex-usage-index", version: codexUsageVersion(definition) },
      {
        module: "sanitized-desktop-state",
        version: readModelVersion(definition),
      },
    ];
    const expectedClaudeVersion = claudeUsageVersion(definition);
    if (expectedClaudeVersion !== null) {
      expectedModuleVersions.push({
        module: "claude-usage-index",
        version: expectedClaudeVersion,
      });
    }
    if (definition.hasExplicitVersions) {
      expectedModuleVersions.push(
        { module: "database-coordinator", version: 1 },
        { module: "desktop-lifecycle", version: 5 },
        { module: "update-state", version: 3 },
      );
    }
    expectedModuleVersions.sort((left, right) => left.module.localeCompare(right.module));
    if (JSON.stringify(moduleVersions) !== JSON.stringify(expectedModuleVersions)) {
      throw new Error(`The module versions are wrong for ${definition.tag}.`);
    }

    const readModel = database
      .query(
        `SELECT schema_version, contract_version, revision, snapshot_json
         FROM sanitized_desktop_state WHERE singleton = 1`,
      )
      .get() as {
      schema_version: number;
      contract_version: number;
      revision: string;
      snapshot_json: string;
    };
    const persistedSnapshot = JSON.parse(readModel.snapshot_json) as {
      revision: string;
      profile: FixtureManifestEntry["expectedState"]["profile"];
      topModelUsage?: null;
    };
    if (
      readModel.schema_version !== readModelVersion(definition) ||
      readModel.contract_version !== readModelContractVersion(definition) ||
      readModel.revision !== entry.expectedState.revision ||
      persistedSnapshot.revision !== entry.expectedState.revision ||
      JSON.stringify(persistedSnapshot.profile) !== JSON.stringify(entry.expectedState.profile) ||
      Object.hasOwn(persistedSnapshot, "topModelUsage") !== definition.hasTopModelUsage
    ) {
      throw new Error(`The read model state is wrong for ${definition.tag}.`);
    }
    const codexToday = persistedSnapshot as {
      providers?: Array<{
        provider?: string;
        usage?: {
          today?: {
            availability?: string;
            observedTokens?: number;
            apiEquivalentCostUsd?: number;
            apiEquivalentCostBasis?: string;
            apiEquivalentCostQuality?: string;
          };
        };
      }>;
    };
    const codexPresentation = codexToday.providers?.find(
      (provider) => provider.provider === "codex",
    );
    const expectedCodexToday = entry.expectedState.usage.codex.at(-1);
    if (
      !expectedCodexToday ||
      codexPresentation?.usage?.today?.availability !== "current" ||
      codexPresentation.usage.today.observedTokens !== expectedCodexToday.observedTokens ||
      codexPresentation.usage.today.apiEquivalentCostUsd !== expectedCodexToday.costUsd ||
      codexPresentation.usage.today.apiEquivalentCostBasis !== expectedCodexToday.pricingBasis ||
      codexPresentation.usage.today.apiEquivalentCostQuality !== "local-only"
    ) {
      throw new Error(`The visible usage state is wrong for ${definition.tag}.`);
    }
    if (
      JSON.stringify(storedUsageFacts(database, "codex")) !==
        JSON.stringify(entry.expectedState.usage.codex) ||
      (definition.hasClaudeUsageIndex &&
        JSON.stringify(storedUsageFacts(database, "claude")) !==
          JSON.stringify(entry.expectedState.usage.claude)) ||
      (!definition.hasClaudeUsageIndex && entry.expectedState.usage.claude.length !== 0)
    ) {
      throw new Error(`The retained usage state is wrong for ${definition.tag}.`);
    }

    const updateColumns = stringRows(
      database,
      "SELECT name FROM pragma_table_info('touchgrassbar_update_state') ORDER BY cid",
    );
    const expectedUpdateColumns =
      definition.updateStateVersion === 1
        ? ["singleton", "last_automatic_check_at", "offered_version", "minimum_required_version"]
        : [
            "singleton",
            "automatic_checks_enabled",
            "last_automatic_check_at",
            "offered_version",
            "minimum_required_version",
          ];
    if (JSON.stringify(updateColumns) !== JSON.stringify(expectedUpdateColumns)) {
      throw new Error(`The update state columns are wrong for ${definition.tag}.`);
    }
    const hasAutomaticChecks = updateColumns.includes("automatic_checks_enabled");
    if (hasAutomaticChecks !== definition.updateStateVersion >= 2) {
      throw new Error(`The update state shape is wrong for ${definition.tag}.`);
    }
    const updateState = database
      .query(
        `SELECT last_automatic_check_at, offered_version, minimum_required_version
         FROM touchgrassbar_update_state WHERE singleton = 1`,
      )
      .get() as {
      last_automatic_check_at: number;
      offered_version: string;
      minimum_required_version: string;
    };
    if (
      updateState.last_automatic_check_at !== entry.expectedState.lastAutomaticCheckAt ||
      updateState.offered_version !== entry.expectedState.offeredVersion ||
      updateState.minimum_required_version !== entry.expectedState.minimumRequiredVersion
    ) {
      throw new Error(`The update state values are wrong for ${definition.tag}.`);
    }
    if (hasAutomaticChecks) {
      const enabled = Number(
        scalarValue(
          database
            .query(
              `SELECT automatic_checks_enabled
               FROM touchgrassbar_update_state WHERE singleton = 1`,
            )
            .get(),
        ),
      );
      if (Boolean(enabled) !== entry.expectedState.automaticChecksEnabledAfterUpgrade) {
        throw new Error(`The update setting is wrong for ${definition.tag}.`);
      }
    } else if (!entry.expectedState.automaticChecksEnabledAfterUpgrade) {
      throw new Error(`The update default is wrong for ${definition.tag}.`);
    }
  } finally {
    database.close();
  }
}

async function assertNoSidecars(databasePath: string): Promise<void> {
  for (const sidecar of [`${databasePath}-wal`, `${databasePath}-shm`]) {
    try {
      await access(sidecar);
    } catch {
      continue;
    }
    throw new Error(`Fixture sidecar must not exist: ${sidecar}`);
  }
}

function assertSanitizedBytes(bytes: Uint8Array, databasePath: string): void {
  const content = Buffer.from(bytes).toString("latin1");
  const forbidden = [
    "/Users/",
    "/home/",
    "C:\\Users\\",
    "BEGIN PRIVATE KEY",
    "Bearer ",
    "access_token",
    "refresh_token",
    "sk-proj-",
  ];
  for (const marker of forbidden) {
    if (content.includes(marker)) {
      throw new Error(`Fixture contains forbidden private data: ${databasePath}`);
    }
  }
}

async function writeFixture(definition: FixtureDefinition): Promise<FixtureManifestEntry> {
  if (definition.releaseStatus !== "candidate") {
    throw new Error("The generator cannot write an official release fixture.");
  }
  if (localTagIsAvailable(definition.tag)) {
    throw new Error("The generator cannot write a tagged release fixture.");
  }
  const fixtureDirectory = join(fixturesRoot, definition.tag);
  const databasePath = join(fixtureDirectory, databaseName);
  await rm(fixtureDirectory, { recursive: true, force: true });
  await mkdir(fixtureDirectory, { recursive: true });

  const database = new Database(databasePath, { create: true, strict: true });
  try {
    database.exec(`
      PRAGMA page_size = 4096;
      PRAGMA auto_vacuum = NONE;
      PRAGMA journal_mode = WAL;
      PRAGMA synchronous = FULL;
      PRAGMA foreign_keys = ON;
      BEGIN IMMEDIATE;
      CREATE TABLE touchgrassbar_schema_versions (
        module TEXT PRIMARY KEY,
        version INTEGER NOT NULL CHECK (version >= 1)
      );
    `);
    createLifecycleSchema(database, definition);
    createReadModelSchema(database, definition);
    if (definition.hasExplicitVersions) {
      createUsageSyncSchema(database, definition);
    }
    createCodexUsageSchema(database, definition);
    if (definition.hasClaudeUsageIndex) {
      createClaudeUsageSchema(database, definition);
    }
    createUpdateStateSchema(database, definition);
    if (definition.hasExplicitVersions) {
      setModuleVersion(database, "desktop-lifecycle", 5);
      setModuleVersion(database, "update-state", 3);
      setModuleVersion(database, "database-coordinator", 1);
      database.exec("PRAGMA user_version = 7");
    }
    database.exec("COMMIT");
    database.exec("PRAGMA wal_checkpoint(TRUNCATE)");
    database.exec("PRAGMA journal_mode = DELETE");
    database.exec("VACUUM");
  } finally {
    database.close();
  }

  await rm(`${databasePath}-wal`, { force: true });
  await rm(`${databasePath}-shm`, { force: true });
  checkDatabase(databasePath);
  await assertNoSidecars(databasePath);
  const bytes = await readFile(databasePath);
  assertSanitizedBytes(bytes, databasePath);
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  return {
    tag: definition.tag,
    database: `${definition.tag}/${databaseName}`,
    sha256,
    sourceCommit: definition.sourceCommit,
    releaseStatus: definition.releaseStatus,
    sourceSchema: {
      databaseFormat: definition.hasExplicitVersions ? 7 : definition.lifecycleVersion,
      lifecycle: definition.lifecycleVersion,
      sanitizedDesktopState: readModelVersion(definition),
      codexUsageIndex: codexUsageVersion(definition),
      claudeUsageIndex: claudeUsageVersion(definition),
      updateState: definition.updateStateVersion,
      databaseCoordinator: definition.hasExplicitVersions ? 1 : null,
    },
    sourceFeatures: {
      providerSettings: definition.lifecycleVersion === 5,
      automaticUpdateChecks: definition.updateStateVersion >= 2,
      claudeUsageIndex: definition.hasClaudeUsageIndex,
      retainedUsage: true,
      topModelUsage: definition.hasTopModelUsage,
    },
    expectedState: {
      revision: definition.revision,
      profile: {
        status: "ready",
        displayName: profileDisplayName,
        touchGrassId: touchGrassId(definition),
      },
      providerSettingsAfterUpgrade: providerSettingsAfterUpgrade(definition),
      usage: {
        codex: codexUsageFacts(definition),
        claude: claudeUsageFacts(definition),
      },
      automaticChecksEnabledAfterUpgrade: definition.updateStateVersion === 1,
      lastAutomaticCheckAt: updateTimestamp(definition),
      offeredVersion: "0.0.9",
      minimumRequiredVersion: "0.0.3",
    },
  };
}

async function readManifest(): Promise<FixtureManifest> {
  const manifest = JSON.parse(await readFile(manifestPath, "utf8")) as FixtureManifest;
  if (manifest.formatVersion !== 1 || !Array.isArray(manifest.fixtures)) {
    throw new Error("The fixture manifest format is not supported.");
  }
  return manifest;
}

function localTagIsAvailable(tag: string): boolean {
  const reference = `refs/tags/${tag}`;
  const present = spawnSync("git", ["show-ref", "--verify", "--quiet", reference], {
    stdio: "ignore",
  });
  if (present.status === 0) return true;
  if (present.status === 1) return false;
  throw new Error(`The source tag cannot be checked for ${tag}.`);
}

function verifyOfficialSourceTag(entry: FixtureManifestEntry): void {
  if (entry.releaseStatus !== "official" || !localTagIsAvailable(entry.tag)) {
    return;
  }
  const reference = `refs/tags/${entry.tag}`;
  const resolved = spawnSync("git", ["rev-parse", "--verify", `${reference}^{commit}`], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  if (resolved.status !== 0 || resolved.stdout.trim() !== entry.sourceCommit) {
    throw new Error(`The source commit does not match tag ${entry.tag}.`);
  }
}

async function validateStoredFixture(
  definition: FixtureDefinition,
  entry: FixtureManifestEntry,
): Promise<void> {
  const databasePath = join(fixturesRoot, entry.database);
  validateFixtureContents(databasePath, definition, entry);
  const bytes = await readFile(databasePath);
  const digest = createHash("sha256").update(bytes).digest("hex");
  if (digest !== entry.sha256) {
    throw new Error(`The fixture hash does not match for ${definition.tag}.`);
  }
  assertSanitizedBytes(bytes, databasePath);
  checkDatabase(databasePath);
  await assertNoSidecars(databasePath);
  verifyOfficialSourceTag(entry);
}

function findSingleCandidateDefinition(): FixtureDefinition {
  const candidates = definitions.filter((definition) => definition.releaseStatus === "candidate");
  const candidate = candidates[0];
  if (candidates.length !== 1 || candidate === undefined) {
    throw new Error("The fixture definitions must have one candidate.");
  }
  return candidate;
}

async function generate(): Promise<void> {
  await mkdir(fixturesRoot, { recursive: true });
  const existing = await readManifest();
  const currentCandidate = findSingleCandidateDefinition();
  const definitionsByTag = new Map(definitions.map((definition) => [definition.tag, definition]));
  const existingByTag = new Map(existing.fixtures.map((entry) => [entry.tag, entry]));
  if (
    definitionsByTag.size !== definitions.length ||
    existingByTag.size !== existing.fixtures.length
  ) {
    throw new Error("The fixture history has a duplicate tag.");
  }
  for (const entry of existing.fixtures) {
    if (
      entry.releaseStatus === "official" &&
      definitionsByTag.get(entry.tag)?.releaseStatus !== "official"
    ) {
      throw new Error(`Official fixture ${entry.tag} cannot be removed.`);
    }
    if (
      entry.releaseStatus === "candidate" &&
      entry.tag !== currentCandidate.tag &&
      definitionsByTag.get(entry.tag)?.releaseStatus !== "official"
    ) {
      throw new Error(`Old candidate ${entry.tag} needs an explicit disposition.`);
    }
  }

  const fixtures: FixtureManifestEntry[] = [];
  for (const definition of definitions) {
    if (definition.releaseStatus === "candidate") continue;
    const stored = existingByTag.get(definition.tag);
    if (stored === undefined) {
      throw new Error(`Official fixture ${definition.tag} cannot be created.`);
    }
    const preserved =
      stored.releaseStatus === "candidate"
        ? {
            ...stored,
            releaseStatus: "official" as const,
            sourceCommit: definition.sourceCommit,
          }
        : stored;
    await validateStoredFixture(definition, preserved);
    fixtures.push(preserved);
  }
  fixtures.push(await writeFixture(currentCandidate));

  const manifest = {
    formatVersion: 1,
    generatedBy: "scripts/generate-database-fixtures.ts",
    fixtures,
  };
  await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
}

async function validate(): Promise<void> {
  const manifest = await readManifest();
  if (manifest.fixtures.length !== definitions.length) {
    throw new Error("The fixture manifest does not list every release.");
  }
  const manifestTags = new Set(manifest.fixtures.map((entry) => entry.tag));
  if (manifestTags.size !== manifest.fixtures.length) {
    throw new Error("The fixture manifest has a duplicate tag.");
  }
  findSingleCandidateDefinition();
  for (const definition of definitions) {
    const entry = manifest.fixtures.find((candidate) => candidate.tag === definition.tag);
    if (entry === undefined) {
      throw new Error(`The fixture manifest is missing ${definition.tag}.`);
    }
    await validateStoredFixture(definition, entry);
  }
  const allowedRootEntries = new Set([
    "README.md",
    "manifest.json",
    ...definitions.map((definition) => definition.tag),
  ]);
  for (const name of await readdir(fixturesRoot)) {
    if (!allowedRootEntries.has(name)) {
      throw new Error(`Unexpected fixture entry: ${name}`);
    }
  }
}

if (process.argv.includes("--check")) {
  await validate();
  console.log(`Validated ${definitions.length} SQLite release fixtures.`);
} else {
  await generate();
  await validate();
  console.log(`Generated ${definitions.length} SQLite release fixtures.`);
}
