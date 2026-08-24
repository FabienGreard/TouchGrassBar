use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde::Deserialize;

use super::super::{migration::coordinator_backup_path, prepare};
use crate::{
    lifecycle::DesktopLifecycle,
    providers::CodingProvider,
    sanitized::{
        self, ApiEquivalentCostQuality, SanitizedProfileOutcome, UsageCoverage, UsageTotal,
    },
    updater,
};

const CURRENT_DATABASE_FORMAT: i64 = 7;
const CURRENT_MODULE_VERSIONS: &[(&str, i64)] = &[
    ("claude-usage-index", 7),
    ("codex-usage-index", 8),
    ("database-coordinator", 1),
    ("desktop-lifecycle", 5),
    ("sanitized-desktop-state", 7),
    ("update-state", 3),
];
const CURRENT_TABLES: &[&str] = &[
    "claude_usage_daily",
    "claude_usage_files",
    "claude_usage_frames",
    "claude_usage_index_meta",
    "claude_usage_message_supersedes",
    "claude_usage_messages",
    "codex_account_usage_days",
    "codex_account_usage_meta",
    "codex_usage_fast_turns",
    "codex_usage_file_days",
    "codex_usage_file_model_days",
    "codex_usage_file_turns",
    "codex_usage_files",
    "codex_usage_index_meta",
    "codex_usage_token_snapshots",
    "lifecycle_state",
    "provider_settings",
    "sanitized_desktop_state",
    "touchgrassbar_schema_versions",
    "touchgrassbar_update_state_v3",
    "usage_sync_correction_lineage",
    "usage_sync_daily_aggregates",
    "usage_sync_generation_activations",
    "usage_sync_generation_baselines",
    "usage_sync_generations",
    "usage_sync_latest_outbox",
    "usage_sync_provider_settings_outbox",
    "usage_sync_terminal_conflicts",
    "usage_sync_transfer_day_carryovers",
];

static NEXT_FIXTURE_COPY: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseFixtureManifest {
    format_version: u8,
    fixtures: Vec<ReleaseFixture>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseFixture {
    tag: String,
    database: String,
    sha256: String,
    release_status: String,
    source_schema: SourceSchema,
    source_features: SourceFeatures,
    expected_state: ExpectedState,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceSchema {
    database_format: i64,
    lifecycle: i64,
    sanitized_desktop_state: i64,
    codex_usage_index: i64,
    claude_usage_index: Option<i64>,
    update_state: i64,
    database_coordinator: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceFeatures {
    claude_usage_index: bool,
    top_model_usage: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedState {
    revision: String,
    profile: ExpectedProfile,
    provider_settings_after_upgrade: ExpectedProviderSettings,
    automatic_checks_enabled_after_upgrade: bool,
    last_automatic_check_at: i64,
    offered_version: String,
    minimum_required_version: String,
    usage: ExpectedUsage,
}

#[derive(Debug, Deserialize)]
struct ExpectedUsage {
    codex: Vec<UsageFact>,
    claude: Vec<UsageFact>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct UsageFact {
    day: String,
    observed_tokens: i64,
    priced_tokens: i64,
    cost_usd: f64,
    complete: bool,
    pricing_basis: String,
    pricing_fingerprint: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedProfile {
    status: String,
    display_name: String,
    touch_grass_id: String,
}

#[derive(Debug, Deserialize)]
struct ExpectedProviderSettings {
    codex: bool,
    claude: bool,
}

#[derive(Debug, Eq, PartialEq)]
struct ProfileState {
    display_name: String,
    touch_grass_id: String,
}

#[derive(Debug, PartialEq)]
struct LogicalState {
    lifecycle_profile: ProfileState,
    read_model_profile: ProfileState,
    revision: String,
    codex_enabled: bool,
    claude_enabled: bool,
    automatic_checks_enabled: bool,
    last_automatic_check_at: Option<i64>,
    offered_version: Option<String>,
    minimum_required_version: Option<String>,
    visible_codex_usage: VisibleUsage,
    top_model_usage: Option<(Option<String>, u64)>,
    codex_usage: Vec<UsageFact>,
    claude_usage: Vec<UsageFact>,
    row_counts: Vec<(String, i64)>,
}

#[derive(Debug, PartialEq)]
struct VisibleUsage {
    observed_tokens: u64,
    cost_usd: f64,
    coverage: UsageCoverage,
    pricing_basis: String,
    pricing_quality: ApiEquivalentCostQuality,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum DurableValue {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

#[derive(Debug, Eq, PartialEq)]
struct ProviderTableFacts {
    table: String,
    columns: Vec<String>,
    row_count: i64,
    rows: Vec<Vec<DurableValue>>,
}

struct FixtureCopy {
    directory: PathBuf,
    database: PathBuf,
}

impl FixtureCopy {
    fn new(tag: &str, source: &Path) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let id = NEXT_FIXTURE_COPY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "touchgrassbar-release-fixture-{}-{tag}-{timestamp}-{id}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("create fixture directory");
        let database = directory.join("touchgrassbar.sqlite3");
        fs::copy(source, &database).expect("copy release fixture");
        Self {
            directory,
            database,
        }
    }
}

impl Drop for FixtureCopy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn every_release_fixture_upgrades_and_reopens_without_loss() {
    let fixtures_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("releases");
    let manifest: ReleaseFixtureManifest = serde_json::from_slice(
        &fs::read(fixtures_root.join("manifest.json")).expect("read release fixture manifest"),
    )
    .expect("parse release fixture manifest");
    assert_eq!(manifest.format_version, 1);
    assert!(!manifest.fixtures.is_empty());

    for fixture in manifest.fixtures {
        assert!(valid_release_tag(&fixture.tag), "invalid tag");
        assert_eq!(
            fixture.expected_state.profile.status, "ready",
            "{}",
            fixture.tag
        );
        assert_eq!(fixture.sha256.len(), 64, "{}", fixture.tag);
        assert!(
            fixture
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "{}",
            fixture.tag
        );
        let source = fixtures_root.join(&fixture.database);
        assert_eq!(
            fixture.database,
            format!("{}/touchgrassbar.sqlite3", fixture.tag),
            "{}",
            fixture.tag
        );
        assert!(
            fs::symlink_metadata(&source)
                .expect("inspect release fixture")
                .file_type()
                .is_file(),
            "{}",
            fixture.tag
        );
        assert_no_sidecars(&source, &fixture.tag);
        assert_fixture_is_private(&source, &fixture.tag);
        if fixture.source_schema.database_format < 7 {
            assert_no_usage_sync_tables(&source, &fixture.tag);
        }

        let working = FixtureCopy::new(&fixture.tag, &source);
        let source_provider_facts = provider_facts(&working.database);
        assert_source_provider_concepts(&source_provider_facts, &fixture);
        let prepared = prepare(&working.database)
            .unwrap_or_else(|error| panic!("{} did not prepare: {error:?}", fixture.tag));
        assert_eq!(
            prepared.path(),
            working.database.as_path(),
            "{}",
            fixture.tag
        );
        let first_state = observe_state(&working.database);
        assert_expected_state(&first_state, &fixture);
        assert_provider_facts_preserved(&working.database, &source_provider_facts, &fixture);
        assert_current_database(&working.database, &fixture.tag);
        let first_backups = backup_inventory(&working.directory);
        if source_requires_upgrade(&fixture.source_schema) {
            let names = first_backups
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(first_backups.len(), 1, "{}: {names:?}", fixture.tag);
            let expected_backup = coordinator_backup_path(&working.database);
            let expected_name = expected_backup
                .file_name()
                .and_then(|name| name.to_str())
                .expect("coordinator backup file name");
            assert_eq!(first_backups[0].0, expected_name, "{}", fixture.tag);
        } else {
            assert!(first_backups.is_empty(), "{}", fixture.tag);
        }

        drop(prepared);
        let reopened = prepare(&working.database)
            .unwrap_or_else(|error| panic!("{} did not reopen: {error:?}", fixture.tag));
        assert_eq!(
            reopened.path(),
            working.database.as_path(),
            "{}",
            fixture.tag
        );
        let reopened_state = observe_state(&working.database);
        assert_eq!(reopened_state, first_state, "{}", fixture.tag);
        assert_provider_facts_preserved(&working.database, &source_provider_facts, &fixture);
        assert_eq!(
            backup_inventory(&working.directory),
            first_backups,
            "{}",
            fixture.tag
        );
        assert_current_database(&working.database, &fixture.tag);
    }
}

fn source_requires_upgrade(source: &SourceSchema) -> bool {
    source.database_format != CURRENT_DATABASE_FORMAT
        || source.lifecycle != 5
        || source.sanitized_desktop_state != 7
        || source.codex_usage_index != 8
        || source.claude_usage_index != Some(7)
        || source.update_state != 3
        || source.database_coordinator != Some(1)
}

fn valid_release_tag(tag: &str) -> bool {
    let Some(version) = tag.strip_prefix('v') else {
        return false;
    };
    let mut components = version.split('.');
    let (Some(major), Some(minor), Some(patch)) =
        (components.next(), components.next(), components.next())
    else {
        return false;
    };
    let valid_component = |component: &str| {
        !component.is_empty()
            && component.bytes().all(|byte| byte.is_ascii_digit())
            && (component == "0" || !component.starts_with('0'))
            && component.parse::<u64>().is_ok()
    };
    components.next().is_none() && [major, minor, patch].into_iter().all(valid_component)
}

fn provider_facts(path: &Path) -> Vec<ProviderTableFacts> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open provider facts");
    provider_table_names(&connection)
        .into_iter()
        .map(|table| {
            let columns = provider_table_columns(&connection, &table);
            provider_table_facts(&connection, &table, columns)
        })
        .collect()
}

fn assert_source_provider_concepts(source: &[ProviderTableFacts], fixture: &ReleaseFixture) {
    let has_table = |table: &&str| source.iter().any(|facts| facts.table == *table);
    let required_codex_tables = [
        "codex_account_usage_days",
        "codex_account_usage_meta",
        "codex_usage_file_days",
        "codex_usage_file_model_days",
        "codex_usage_files",
        "codex_usage_index_meta",
    ];
    assert!(
        required_codex_tables.iter().all(has_table),
        "{} has incomplete source Codex facts",
        fixture.tag
    );
    if fixture.release_status == "candidate" {
        assert!(
            CURRENT_TABLES
                .iter()
                .filter(|table| table.starts_with("codex_"))
                .all(has_table),
            "{} has an incomplete current Codex schema",
            fixture.tag
        );
    }
    let mut claude_tables = CURRENT_TABLES
        .iter()
        .filter(|table| table.starts_with("claude_"));
    if fixture.source_features.claude_usage_index {
        assert!(
            claude_tables.clone().all(has_table),
            "{} has incomplete source Claude facts",
            fixture.tag
        );
    } else {
        assert!(
            claude_tables.all(|table| !has_table(table)),
            "{} contains a historical Claude concept",
            fixture.tag
        );
    }
}

fn assert_provider_facts_preserved(
    path: &Path,
    source: &[ProviderTableFacts],
    fixture: &ReleaseFixture,
) {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open migrated provider facts");
    let current_tables = provider_table_names(&connection);
    for expected in source {
        assert!(
            current_tables.contains(&expected.table),
            "{} removed provider table {}",
            fixture.tag,
            expected.table
        );
        if fixture.source_schema.codex_usage_index < 6
            && matches!(
                expected.table.as_str(),
                "codex_usage_file_days" | "codex_usage_file_model_days" | "codex_usage_files"
            )
        {
            assert_eq!(
                table_row_count(&connection, &expected.table),
                0,
                "{} retained a stale rebuildable Codex index",
                fixture.tag
            );
            continue;
        }
        if fixture.source_schema.codex_usage_index < 8
            && expected.table == "codex_account_usage_meta"
        {
            let actual = provider_table_facts(
                &connection,
                &expected.table,
                vec!["singleton".to_owned(), "refreshed_at".to_owned()],
            );
            assert_eq!(actual.row_count, expected.row_count, "{}", fixture.tag);
            assert_eq!(actual.rows, expected.rows, "{}", fixture.tag);
            continue;
        }
        let actual = provider_table_facts(&connection, &expected.table, expected.columns.clone());
        assert_eq!(
            actual.row_count, expected.row_count,
            "{} changed {} row count",
            fixture.tag, expected.table
        );
        assert_eq!(
            actual.rows, expected.rows,
            "{} changed {} durable facts",
            fixture.tag, expected.table
        );
        if fixture.source_schema.codex_usage_index < 8
            && expected.table == "codex_account_usage_days"
        {
            let timestamp_mismatches = connection
                .query_row(
                    "SELECT COUNT(*)
                     FROM codex_account_usage_days AS account_day
                     JOIN codex_account_usage_meta AS account_meta
                       ON account_meta.singleton = 1
                     WHERE account_day.observed_at != account_meta.refreshed_at",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("count migrated account timestamp mismatches");
            assert_eq!(timestamp_mismatches, 0, "{}", fixture.tag);
        }
    }

    for table in current_tables {
        if source.iter().any(|expected| expected.table == table) {
            continue;
        }
        assert_eq!(
            table_row_count(&connection, &table),
            0,
            "{} invented durable facts in historical provider table {table}",
            fixture.tag
        );
    }
}

fn provider_table_names(connection: &Connection) -> Vec<String> {
    connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table'
               AND (name GLOB 'codex_*' OR name GLOB 'claude_*')
             ORDER BY name",
        )
        .expect("prepare provider table query")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query provider tables")
        .collect::<Result<Vec<_>, _>>()
        .expect("read provider tables")
}

fn provider_table_columns(connection: &Connection, table: &str) -> Vec<String> {
    assert!(safe_identifier(table), "unsafe provider table name");
    connection
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .expect("prepare provider column query")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query provider columns")
        .map(|column| {
            let column = column.expect("read provider column");
            assert!(safe_identifier(&column), "unsafe provider column name");
            column
        })
        .collect()
}

fn provider_table_facts(
    connection: &Connection,
    table: &str,
    columns: Vec<String>,
) -> ProviderTableFacts {
    assert!(safe_identifier(table), "unsafe provider table name");
    assert!(!columns.is_empty(), "provider table has no columns");
    assert!(
        columns.iter().all(|column| safe_identifier(column)),
        "unsafe provider column name"
    );
    let selection = columns
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = connection
        .prepare(&format!("SELECT {selection} FROM \"{table}\""))
        .expect("prepare provider fact query");
    let mut rows = statement
        .query_map([], |row| {
            (0..columns.len())
                .map(|index| row.get_ref(index).map(durable_value))
                .collect::<Result<Vec<_>, _>>()
        })
        .expect("query provider facts")
        .collect::<Result<Vec<_>, _>>()
        .expect("read provider facts");
    rows.sort();
    let row_count = table_row_count(connection, table);
    assert_eq!(
        usize::try_from(row_count).expect("nonnegative provider row count"),
        rows.len(),
        "provider row count mismatch"
    );
    ProviderTableFacts {
        table: table.to_owned(),
        columns,
        row_count,
        rows,
    }
}

fn table_row_count(connection: &Connection, table: &str) -> i64 {
    assert!(safe_identifier(table), "unsafe provider table name");
    connection
        .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
            row.get(0)
        })
        .expect("count provider rows")
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn durable_value(value: ValueRef<'_>) -> DurableValue {
    match value {
        ValueRef::Null => DurableValue::Null,
        ValueRef::Integer(value) => DurableValue::Integer(value),
        ValueRef::Real(value) => DurableValue::Real(value.to_bits()),
        ValueRef::Text(value) => DurableValue::Text(value.to_vec()),
        ValueRef::Blob(value) => DurableValue::Blob(value.to_vec()),
    }
}

fn observe_state(path: &Path) -> LogicalState {
    let lifecycle = DesktopLifecycle::open(path).expect("open lifecycle state");
    let lifecycle_profile = profile_state(&lifecycle.sanitized_profile_outcome());
    let codex_enabled = lifecycle.is_provider_enabled(CodingProvider::Codex);
    let claude_enabled = lifecycle.is_provider_enabled(CodingProvider::Claude);
    drop(lifecycle);

    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open read model");
    let read_model = sanitized::read_database_state(&connection).expect("read sanitized state");
    let read_model_profile = profile_state(&read_model.profile);
    let revision = read_model.revision.clone();
    let codex_today = &read_model
        .provider(CodingProvider::Codex)
        .expect("Codex presentation")
        .usage
        .today;
    let UsageTotal::Current {
        observed_tokens,
        api_equivalent_cost_usd: Some(cost_usd),
        coverage,
        api_equivalent_cost_basis: Some(pricing_basis),
        api_equivalent_cost_quality: Some(pricing_quality),
        ..
    } = codex_today
    else {
        panic!("Codex fixture usage is not current and priced");
    };
    let visible_codex_usage = VisibleUsage {
        observed_tokens: *observed_tokens,
        cost_usd: *cost_usd,
        coverage: *coverage,
        pricing_basis: pricing_basis.clone(),
        pricing_quality: *pricing_quality,
    };
    let top_model_usage = read_model
        .top_model_usage
        .map(|usage| (usage.model, usage.observed_tokens));
    let codex_usage = usage_facts(&connection, "codex");
    let claude_usage = usage_facts(&connection, "claude");
    let row_counts = CURRENT_TABLES
        .iter()
        .map(|table| {
            let count = connection
                .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count table rows");
            ((*table).to_owned(), count)
        })
        .collect();
    drop(connection);

    let update = updater::read_database_state(path).expect("read update state");
    LogicalState {
        lifecycle_profile,
        read_model_profile,
        revision,
        codex_enabled,
        claude_enabled,
        automatic_checks_enabled: update.automatic_checks_enabled,
        last_automatic_check_at: update.last_automatic_check_at,
        offered_version: update.offered_version,
        minimum_required_version: update.minimum_required_version,
        visible_codex_usage,
        top_model_usage,
        codex_usage,
        claude_usage,
        row_counts,
    }
}

fn usage_facts(connection: &Connection, provider: &str) -> Vec<UsageFact> {
    let query = match provider {
        "codex" => {
            "SELECT daily.day, daily.observed_tokens, daily.priced_tokens,
                    daily.cost_usd, daily.complete, model.pricing_basis,
                    daily.pricing_fingerprint
             FROM codex_usage_file_days AS daily
             JOIN codex_usage_file_model_days AS model
               ON model.path = daily.path AND model.day = daily.day
             ORDER BY daily.day"
        }
        "claude" => {
            "SELECT day, observed_tokens, priced_tokens, cost_usd,
                    coverage = 'complete', pricing_basis, pricing_fingerprint
             FROM claude_usage_daily ORDER BY day"
        }
        _ => panic!("unknown usage provider"),
    };
    connection
        .prepare(query)
        .expect("prepare usage query")
        .query_map([], |row| {
            Ok(UsageFact {
                day: row.get(0)?,
                observed_tokens: row.get(1)?,
                priced_tokens: row.get(2)?,
                cost_usd: row.get(3)?,
                complete: row.get(4)?,
                pricing_basis: row.get(5)?,
                pricing_fingerprint: row.get(6)?,
            })
        })
        .expect("query usage facts")
        .collect::<Result<Vec<_>, _>>()
        .expect("read usage facts")
}

fn profile_state(profile: &SanitizedProfileOutcome) -> ProfileState {
    match profile {
        SanitizedProfileOutcome::Ready {
            display_name,
            touch_grass_id,
        } => ProfileState {
            display_name: display_name.clone(),
            touch_grass_id: touch_grass_id.clone(),
        },
        SanitizedProfileOutcome::NotAuthorized => panic!("fixture profile is not authorized"),
        SanitizedProfileOutcome::ProfilePending => panic!("fixture profile is pending"),
    }
}

fn assert_expected_state(state: &LogicalState, fixture: &ReleaseFixture) {
    let expected_profile = ProfileState {
        display_name: fixture.expected_state.profile.display_name.clone(),
        touch_grass_id: fixture.expected_state.profile.touch_grass_id.clone(),
    };
    assert_eq!(
        state.lifecycle_profile, expected_profile,
        "{} lifecycle profile",
        fixture.tag
    );
    assert_eq!(
        state.read_model_profile, expected_profile,
        "{} read model profile",
        fixture.tag
    );
    assert_eq!(
        state.revision, fixture.expected_state.revision,
        "{}",
        fixture.tag
    );
    assert_eq!(
        state.codex_enabled, fixture.expected_state.provider_settings_after_upgrade.codex,
        "{}",
        fixture.tag
    );
    assert_eq!(
        state.claude_enabled,
        fixture
            .expected_state
            .provider_settings_after_upgrade
            .claude,
        "{}",
        fixture.tag
    );
    assert_eq!(
        state.automatic_checks_enabled,
        fixture
            .expected_state
            .automatic_checks_enabled_after_upgrade,
        "{}",
        fixture.tag
    );
    assert_eq!(
        state.last_automatic_check_at,
        Some(fixture.expected_state.last_automatic_check_at),
        "{}",
        fixture.tag
    );
    assert_eq!(
        state.offered_version.as_deref(),
        Some(fixture.expected_state.offered_version.as_str()),
        "{}",
        fixture.tag
    );
    assert_eq!(
        state.minimum_required_version.as_deref(),
        Some(fixture.expected_state.minimum_required_version.as_str()),
        "{}",
        fixture.tag
    );
    let expected_codex_today = fixture
        .expected_state
        .usage
        .codex
        .last()
        .expect("expected Codex usage");
    assert_eq!(
        state.visible_codex_usage,
        VisibleUsage {
            observed_tokens: u64::try_from(expected_codex_today.observed_tokens)
                .expect("nonnegative observed tokens"),
            cost_usd: expected_codex_today.cost_usd,
            coverage: if expected_codex_today.complete {
                UsageCoverage::Complete
            } else {
                UsageCoverage::Partial
            },
            pricing_basis: expected_codex_today.pricing_basis.clone(),
            pricing_quality: ApiEquivalentCostQuality::LocalOnly,
        },
        "{} visible usage",
        fixture.tag
    );
    let expected_top_model = fixture.source_features.top_model_usage.then(|| {
        (
            Some("GPT 5.2".to_owned()),
            u64::try_from(expected_codex_today.observed_tokens)
                .expect("nonnegative top model tokens"),
        )
    });
    assert_eq!(state.top_model_usage, expected_top_model, "{}", fixture.tag);
    let expected_codex_usage = if fixture.source_schema.codex_usage_index >= 6 {
        fixture.expected_state.usage.codex.as_slice()
    } else {
        &[]
    };
    assert_eq!(
        state.codex_usage, expected_codex_usage,
        "{} Codex usage",
        fixture.tag
    );
    assert_eq!(
        state.claude_usage, fixture.expected_state.usage.claude,
        "{} Claude usage",
        fixture.tag
    );
    assert_eq!(
        !state.claude_usage.is_empty(),
        fixture.source_features.claude_usage_index,
        "{} Claude history",
        fixture.tag
    );
}

fn assert_current_database(path: &Path, tag: &str) {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open prepared fixture");
    let format = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .expect("read database format");
    assert_eq!(format, CURRENT_DATABASE_FORMAT, "{tag}");
    let versions = connection
        .prepare("SELECT module, version FROM touchgrassbar_schema_versions ORDER BY module")
        .expect("prepare version query")
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .expect("query version vector")
        .collect::<Result<Vec<_>, _>>()
        .expect("read version vector");
    let expected = CURRENT_MODULE_VERSIONS
        .iter()
        .map(|(module, version)| ((*module).to_owned(), *version))
        .collect::<Vec<_>>();
    assert_eq!(versions, expected, "{tag}");
}

fn assert_no_usage_sync_tables(path: &Path, tag: &str) {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open historical fixture");
    let tables = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND (
               lower(name) LIKE '%usage%sync%'
               OR lower(name) LIKE '%pending%usage%snapshot%'
               OR lower(name) LIKE '%active%mac%generation%'
               OR lower(name) LIKE '%ack%floor%'
               OR lower(name) LIKE '%correction%lineage%'
             )
             ORDER BY name",
        )
        .expect("prepare usage sync table query")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query usage sync tables")
        .collect::<Result<Vec<_>, _>>()
        .expect("read usage sync tables");
    assert!(tables.is_empty(), "{tag} has invented tables: {tables:?}");
}

fn assert_fixture_is_private(path: &Path, tag: &str) {
    let bytes = fs::read(path).expect("read fixture bytes");
    for marker in [
        b"/Users/".as_slice(),
        b"/home/".as_slice(),
        b"C:\\Users\\".as_slice(),
        b"BEGIN PRIVATE KEY".as_slice(),
        b"Bearer ".as_slice(),
        b"access_token".as_slice(),
        b"refresh_token".as_slice(),
        b"sk-proj-".as_slice(),
    ] {
        assert!(
            !bytes.windows(marker.len()).any(|window| window == marker),
            "{tag} contains a private-data marker"
        );
    }
}

fn assert_no_sidecars(path: &Path, tag: &str) {
    for suffix in ["-wal", "-shm"] {
        assert!(!sidecar_path(path, suffix).exists(), "{tag} has {suffix}");
    }
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

fn backup_inventory(directory: &Path) -> Vec<(String, Vec<u8>)> {
    let mut backups = fs::read_dir(directory)
        .expect("read fixture directory")
        .map(|entry| entry.expect("read fixture entry"))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (name.ends_with(".backup") || name.ends_with(".backup.partial"))
                .then(|| (name, fs::read(entry.path()).expect("read fixture backup")))
        })
        .collect::<Vec<_>>();
    backups.sort_by(|left, right| left.0.cmp(&right.0));
    assert!(
        backups.iter().all(|(name, _)| !name.ends_with(".partial")),
        "partial fixture backup remains"
    );
    backups
}
