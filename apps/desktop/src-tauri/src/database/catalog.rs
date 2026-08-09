use crate::{lifecycle, providers, sanitized, updater};

pub(super) const DATABASE_FORMAT_VERSION: i64 = 6;
pub(super) const COORDINATOR_SCHEMA_MODULE: &str = "database-coordinator";
pub(super) const COORDINATOR_SCHEMA_VERSION: i64 = 1;

pub(super) const MODULES: &[(&str, i64)] = &[
    (
        lifecycle::DATABASE_SCHEMA_MODULE,
        lifecycle::DATABASE_SCHEMA_VERSION,
    ),
    (
        sanitized::READ_MODEL_SCHEMA_MODULE,
        sanitized::READ_MODEL_SCHEMA_VERSION,
    ),
    (
        providers::CODEX_USAGE_SCHEMA_MODULE,
        providers::CODEX_USAGE_SCHEMA_VERSION,
    ),
    (
        providers::CLAUDE_USAGE_SCHEMA_MODULE,
        providers::CLAUDE_USAGE_SCHEMA_VERSION,
    ),
    (
        updater::DATABASE_SCHEMA_MODULE,
        updater::DATABASE_SCHEMA_VERSION,
    ),
    (COORDINATOR_SCHEMA_MODULE, COORDINATOR_SCHEMA_VERSION),
];

pub(super) const TABLES: &[&str] = &[
    "claude_usage_daily",
    "claude_usage_files",
    "claude_usage_frames",
    "claude_usage_index_meta",
    "claude_usage_message_supersedes",
    "claude_usage_messages",
    "codex_account_usage_days",
    "codex_account_usage_meta",
    "codex_usage_file_days",
    "codex_usage_file_model_days",
    "codex_usage_files",
    "codex_usage_index_meta",
    "lifecycle_state",
    "provider_settings",
    "sanitized_desktop_state",
    "touchgrassbar_schema_versions",
    "touchgrassbar_update_state_v3",
];

pub(super) const INDEXES: &[&str] = &[
    "claude_usage_frames_by_day",
    "claude_usage_messages_by_day",
    "claude_usage_messages_by_message",
    "claude_usage_messages_by_superseded_frame",
    "claude_usage_supersedes_by_superseded_frame",
    "codex_usage_model_days_by_day",
    "codex_usage_unpriced_model_days",
];

pub(super) const VIEWS: &[&str] = &["touchgrassbar_update_state"];

// This catalog contains every distinct object definition emitted by releases
// v0.0.3 through v0.0.9. Case and whitespace do not affect a definition. Any
// other SQL is a new database shape and needs an explicit migration contract.
pub(super) const KNOWN_OBJECT_DEFINITIONS: &[&str] = &[
    "createindexclaude_usage_frames_by_dayonclaude_usage_frames(day)",
    "createindexclaude_usage_messages_by_dayonclaude_usage_messages(day)",
    "createindexclaude_usage_messages_by_messageonclaude_usage_messages(message_key)",
    concat!(
        "createindexclaude_usage_messages_by_superseded_frame",
        "onclaude_usage_messages(supersedes_frame_key)"
    ),
    concat!(
        "createindexclaude_usage_supersedes_by_superseded_frame",
        "onclaude_usage_message_supersedes(superseded_frame_key,parser_version)"
    ),
    "createindexcodex_usage_model_days_by_dayoncodex_usage_file_model_days(day)",
    concat!(
        "createindexcodex_usage_unpriced_model_days",
        "oncodex_usage_file_model_days(day,model,cache_write_input_tokens)",
        "wherecost_usdisnull"
    ),
    concat!(
        "createtableclaude_usage_daily(",
        "daytextprimarykeynotnull,",
        "observed_tokensintegernotnull,",
        "coveragetextnotnullcheck(coveragein('complete','partial')),",
        "observed_throughtextnotnull,",
        "revisionintegernotnullcheck(revision>=1),",
        "priced_tokensintegernotnulldefault0,",
        "cost_usdreal,pricing_basistext,pricing_fingerprinttext)"
    ),
    concat!(
        "createtableclaude_usage_files(",
        "pathtextprimarykeynotnull,file_identitytextnotnull,",
        "size_bytesintegernotnull,modified_nsintegernotnull,",
        "parsed_offsetintegernotnull,resume_anchortext,",
        "parser_versionintegernotnull,completion_statetextnotnull)"
    ),
    concat!(
        "createtableclaude_usage_frames(",
        "frame_keytextprimarykeynotnull,daytextnotnull,",
        "observed_attextnotnull,parser_versionintegernotnull)"
    ),
    concat!(
        "createtableclaude_usage_index_meta(",
        "keytextprimarykeynotnull,valuetextnotnull)"
    ),
    concat!(
        "createtableclaude_usage_message_supersedes(",
        "replacement_frame_keytextnotnull,superseded_frame_keytextnotnull,",
        "parser_versionintegernotnull,",
        "primarykey(replacement_frame_key,superseded_frame_key),",
        "foreignkey(replacement_frame_key)",
        "referencesclaude_usage_frames(frame_key)ondeletecascade)"
    ),
    concat!(
        "createtable\"claude_usage_message_supersedes\"(",
        "replacement_frame_keytextnotnull,superseded_frame_keytextnotnull,",
        "parser_versionintegernotnull,",
        "primarykey(replacement_frame_key,superseded_frame_key),",
        "foreignkey(replacement_frame_key)",
        "referencesclaude_usage_frames(frame_key)ondeletecascade)"
    ),
    concat!(
        "createtableclaude_usage_messages(",
        "frame_keytextprimarykeynotnull,supersedes_frame_keytext,",
        "message_keytextnotnull,daytextnotnull,observed_attextnotnull,",
        "modeltextnotnull,input_tokensintegernotnull,",
        "cache_creation_input_tokensintegernotnull,",
        "cache_read_input_tokensintegernotnull,output_tokensintegernotnull,",
        "cache_creation_5m_input_tokensinteger,",
        "cache_creation_1h_input_tokensinteger,service_tiertext,",
        "inference_geotext,speedtext,web_search_requestsinteger,",
        "web_fetch_requestsinteger,code_execution_requestsinteger,",
        "has_unknown_paid_server_toolintegernotnull,",
        "observed_tokensintegernotnull,completeintegernotnull,",
        "parser_versionintegernotnull)"
    ),
    concat!(
        "createtablecodex_account_usage_days(",
        "daytextprimarykeynotnull,tokensintegernotnull)"
    ),
    concat!(
        "createtablecodex_account_usage_meta(",
        "singletonintegerprimarykeynotnullcheck(singleton=1),",
        "observed_attextnotnull)"
    ),
    concat!(
        "createtablecodex_usage_file_days(",
        "pathtextnotnull,daytextnotnull,observed_tokensintegernotnull,",
        "priced_tokensintegernotnull,cost_usdrealnotnull,",
        "completeintegernotnull,observed_throughtextnotnull,",
        "priced_observed_throughtext,pricing_fingerprinttext,",
        "primarykey(path,day),",
        "foreignkey(path)referencescodex_usage_files(path)ondeletecascade)"
    ),
    concat!(
        "createtablecodex_usage_file_model_days(",
        "pathtextnotnull,daytextnotnull,modeltextnotnull,",
        "pricing_input_tokensintegernotnull,input_tokensintegernotnull,",
        "cached_input_tokensintegernotnull,",
        "cache_write_input_tokensintegernotnull,",
        "output_tokensintegernotnull,reasoning_output_tokensintegernotnull,",
        "observed_tokensintegernotnull,cost_usdreal,pricing_basistext,",
        "pricing_fingerprinttext,completeintegernotnull,",
        "observed_throughtextnotnull,",
        "primarykey(path,day,model,pricing_input_tokens),",
        "foreignkey(path)referencescodex_usage_files(path)ondeletecascade)"
    ),
    concat!(
        "createtablecodex_usage_files(",
        "pathtextprimarykeynotnull,file_identitytextnotnull,",
        "size_bytesintegernotnull,modified_nsintegernotnull,",
        "parsed_offsetintegernotnull,parsed_prefix_anchortext,",
        "parser_versionintegernotnull,completion_statetextnotnull,",
        "deferred_until_daytext,active_modeltext,",
        "baseline_is_inheritedinteger,history_start_ordinalinteger,",
        "record_ordinalintegernotnulldefault0,",
        "usage_excludedintegernotnulldefault0,",
        "schema_supportedintegernotnull,previous_inputinteger,",
        "previous_cached_inputinteger,previous_cache_write_inputinteger,",
        "previous_outputinteger,previous_reasoning_outputinteger,",
        "previous_totalinteger)"
    ),
    concat!(
        "createtablecodex_usage_index_meta(",
        "keytextprimarykeynotnull,valuetextnotnull)"
    ),
    concat!(
        "createtablelifecycle_state(",
        "singletonintegerprimarykeycheck(singleton=1),",
        "bootstrap_completedintegernotnullcheck(bootstrap_completedin(0,1)),",
        "profile_provisioningtextnotnullcheck(",
        "profile_provisioningin('not-authorized','profile-pending','ready')),",
        "public_participation_authorizedintegernotnullcheck(",
        "public_participation_authorizedin(0,1)),",
        "profile_retry_pendingintegernotnullcheck(",
        "profile_retry_pendingin(0,1)),",
        "backfill_window_daysintegercheck(backfill_window_days=30),",
        "display_nametextcheck(",
        "display_nameisnullor(length(trim(display_name))between1and40)),",
        "touch_grass_idtext,",
        "recovery_disclosure_pendingintegernotnulldefault0check(",
        "recovery_disclosure_pendingin(0,1)))"
    ),
    concat!(
        "createtableprovider_settings(",
        "providertextprimarykeycheck(providerin('codex','claude')),",
        "enabledintegernotnullcheck(enabledin(0,1)))"
    ),
    concat!(
        "createtablesanitized_desktop_state(",
        "singletonintegerprimarykeycheck(singleton=1),",
        "schema_versionintegernotnullcheck(schema_version=4),",
        "contract_versionintegernotnullcheck(contract_version=3),",
        "revisiontextnotnullcheck(",
        "length(revision)>0andrevisionnotglob'*[^0-9]*'),",
        "snapshot_jsontextnotnull)"
    ),
    concat!(
        "createtablesanitized_desktop_state(",
        "singletonintegerprimarykeycheck(singleton=1),",
        "schema_versionintegernotnullcheck(schema_version=5),",
        "contract_versionintegernotnullcheck(contract_version=3),",
        "revisiontextnotnullcheck(",
        "length(revision)>0andrevisionnotglob'*[^0-9]*'),",
        "snapshot_jsontextnotnull)"
    ),
    concat!(
        "createtabletouchgrassbar_schema_versions(",
        "moduletextprimarykey,versionintegernotnullcheck(version>=1))"
    ),
    concat!(
        "createtabletouchgrassbar_update_state(",
        "singletonintegerprimarykeycheck(singleton=1),",
        "last_automatic_check_atinteger,deferred_versiontext)"
    ),
    concat!(
        "createtabletouchgrassbar_update_state(",
        "singletonintegerprimarykeycheck(singleton=1),",
        "last_automatic_check_atinteger,",
        "offered_versiontextcheck(",
        "offered_versionisnullorlength(offered_version)between1and64),",
        "minimum_required_versiontextcheck(",
        "minimum_required_versionisnullor",
        "length(minimum_required_version)between1and64))"
    ),
    concat!(
        "createtabletouchgrassbar_update_state(",
        "singletonintegerprimarykeycheck(singleton=1),",
        "automatic_checks_enabledintegernotnulldefault1check(",
        "automatic_checks_enabledin(0,1)),last_automatic_check_atinteger,",
        "offered_versiontextcheck(",
        "offered_versionisnullorlength(offered_version)between1and64),",
        "minimum_required_versiontextcheck(",
        "minimum_required_versionisnullor",
        "length(minimum_required_version)between1and64))"
    ),
    concat!(
        "createtabletouchgrassbar_update_state_v3(",
        "singletonintegerprimarykeycheck(singleton=1),",
        "automatic_checks_enabledintegernotnulldefault1check(",
        "automatic_checks_enabledin(0,1)),last_automatic_check_atinteger,",
        "offered_versiontextcheck(",
        "offered_versionisnullorlength(offered_version)between1and64),",
        "minimum_required_versiontextcheck(",
        "minimum_required_versionisnullor",
        "length(minimum_required_version)between1and64))"
    ),
    concat!(
        "createtable\"touchgrassbar_update_state_v3\"(",
        "singletonintegerprimarykeycheck(singleton=1),",
        "automatic_checks_enabledintegernotnulldefault1check(",
        "automatic_checks_enabledin(0,1)),last_automatic_check_atinteger,",
        "offered_versiontextcheck(",
        "offered_versionisnullorlength(offered_version)between1and64),",
        "minimum_required_versiontextcheck(",
        "minimum_required_versionisnullor",
        "length(minimum_required_version)between1and64))"
    ),
    concat!(
        "createtabletouchgrassbar_update_state_v3(",
        "singletonintegerprimarykeycheck(singleton=1),",
        "last_automatic_check_atinteger,",
        "offered_versiontextcheck(",
        "offered_versionisnullorlength(offered_version)between1and64),",
        "minimum_required_versiontextcheck(",
        "minimum_required_versionisnullor",
        "length(minimum_required_version)between1and64),",
        "automatic_checks_enabledintegernotnulldefault1check(",
        "automatic_checks_enabledin(0,1)))"
    ),
    concat!(
        "createtable\"touchgrassbar_update_state_v3\"(",
        "singletonintegerprimarykeycheck(singleton=1),",
        "last_automatic_check_atinteger,",
        "offered_versiontextcheck(",
        "offered_versionisnullorlength(offered_version)between1and64),",
        "minimum_required_versiontextcheck(",
        "minimum_required_versionisnullor",
        "length(minimum_required_version)between1and64),",
        "automatic_checks_enabledintegernotnulldefault1check(",
        "automatic_checks_enabledin(0,1)))"
    ),
    concat!(
        "createviewtouchgrassbar_update_stateasselect",
        "singleton,automatic_checks_enabled,last_automatic_check_at,",
        "offered_version,minimum_required_version",
        "fromtouchgrassbar_update_state_v3"
    ),
];

pub(super) const PRIMARY_KEYS: &[(&str, &[&str])] = &[
    ("claude_usage_daily", &["day"]),
    ("claude_usage_files", &["path"]),
    ("claude_usage_frames", &["frame_key"]),
    ("claude_usage_index_meta", &["key"]),
    (
        "claude_usage_message_supersedes",
        &["replacement_frame_key", "superseded_frame_key"],
    ),
    ("claude_usage_messages", &["frame_key"]),
    ("codex_account_usage_days", &["day"]),
    ("codex_account_usage_meta", &["singleton"]),
    ("codex_usage_file_days", &["path", "day"]),
    (
        "codex_usage_file_model_days",
        &["path", "day", "model", "pricing_input_tokens"],
    ),
    ("codex_usage_files", &["path"]),
    ("codex_usage_index_meta", &["key"]),
    ("lifecycle_state", &["singleton"]),
    ("provider_settings", &["provider"]),
    ("sanitized_desktop_state", &["singleton"]),
    ("touchgrassbar_schema_versions", &["module"]),
    ("touchgrassbar_update_state_v3", &["singleton"]),
];

pub(super) const NULLABLE_COLUMNS: &[(&str, &[&str])] = &[
    (
        "claude_usage_daily",
        &["cost_usd", "pricing_basis", "pricing_fingerprint"],
    ),
    ("claude_usage_files", &["resume_anchor"]),
    (
        "claude_usage_messages",
        &[
            "supersedes_frame_key",
            "cache_creation_5m_input_tokens",
            "cache_creation_1h_input_tokens",
            "service_tier",
            "inference_geo",
            "speed",
            "web_search_requests",
            "web_fetch_requests",
            "code_execution_requests",
        ],
    ),
    (
        "codex_usage_file_days",
        &["priced_observed_through", "pricing_fingerprint"],
    ),
    (
        "codex_usage_file_model_days",
        &["cost_usd", "pricing_basis", "pricing_fingerprint"],
    ),
    (
        "codex_usage_files",
        &[
            "parsed_prefix_anchor",
            "deferred_until_day",
            "active_model",
            "baseline_is_inherited",
            "history_start_ordinal",
            "previous_input",
            "previous_cached_input",
            "previous_cache_write_input",
            "previous_output",
            "previous_reasoning_output",
            "previous_total",
        ],
    ),
    (
        "lifecycle_state",
        &["backfill_window_days", "display_name", "touch_grass_id"],
    ),
    (
        "touchgrassbar_update_state_v3",
        &[
            "last_automatic_check_at",
            "offered_version",
            "minimum_required_version",
        ],
    ),
];

pub(super) const COLUMN_DEFAULTS: &[(&str, &str, &str)] = &[
    ("claude_usage_daily", "priced_tokens", "0"),
    ("codex_usage_files", "record_ordinal", "0"),
    ("codex_usage_files", "usage_excluded", "0"),
    ("lifecycle_state", "recovery_disclosure_pending", "0"),
    (
        "touchgrassbar_update_state_v3",
        "automatic_checks_enabled",
        "1",
    ),
];

pub(super) const FOREIGN_KEYS: &[(&str, &str, &str, &str)] = &[
    (
        "claude_usage_message_supersedes",
        "replacement_frame_key",
        "claude_usage_frames",
        "frame_key",
    ),
    ("codex_usage_file_days", "path", "codex_usage_files", "path"),
    (
        "codex_usage_file_model_days",
        "path",
        "codex_usage_files",
        "path",
    ),
];

pub(super) const INDEX_DEFINITIONS: &[(&str, &str, &[&str], Option<&str>)] = &[
    (
        "claude_usage_frames_by_day",
        "claude_usage_frames",
        &["day"],
        None,
    ),
    (
        "claude_usage_messages_by_day",
        "claude_usage_messages",
        &["day"],
        None,
    ),
    (
        "claude_usage_messages_by_message",
        "claude_usage_messages",
        &["message_key"],
        None,
    ),
    (
        "claude_usage_messages_by_superseded_frame",
        "claude_usage_messages",
        &["supersedes_frame_key"],
        None,
    ),
    (
        "claude_usage_supersedes_by_superseded_frame",
        "claude_usage_message_supersedes",
        &["superseded_frame_key", "parser_version"],
        None,
    ),
    (
        "codex_usage_model_days_by_day",
        "codex_usage_file_model_days",
        &["day"],
        None,
    ),
    (
        "codex_usage_unpriced_model_days",
        "codex_usage_file_model_days",
        &["day", "model", "cache_write_input_tokens"],
        Some("cost_usd is null"),
    ),
];

pub(super) const TABLE_CHECKS: &[(&str, &[&str])] = &[
    (
        "claude_usage_daily",
        &[
            "check(coveragein('complete','partial'))",
            "check(revision>=1)",
        ],
    ),
    ("codex_account_usage_meta", &["check(singleton=1)"]),
    (
        "lifecycle_state",
        &[
            "check(singleton=1)",
            "check(bootstrap_completedin(0,1))",
            "check(profile_provisioningin('not-authorized','profile-pending','ready'))",
            "check(public_participation_authorizedin(0,1))",
            "check(profile_retry_pendingin(0,1))",
            "check(backfill_window_days=30)",
            "check(display_nameisnullor(length(trim(display_name))between1and40))",
            "check(recovery_disclosure_pendingin(0,1))",
        ],
    ),
    (
        "provider_settings",
        &[
            "check(providerin('codex','claude'))",
            "check(enabledin(0,1))",
        ],
    ),
    (
        "sanitized_desktop_state",
        &[
            "check(singleton=1)",
            "check(schema_version=5)",
            "check(contract_version=3)",
            "check(length(revision)>0andrevisionnotglob'*[^0-9]*')",
        ],
    ),
    ("touchgrassbar_schema_versions", &["check(version>=1)"]),
    (
        "touchgrassbar_update_state_v3",
        &[
            "check(singleton=1)",
            "check(automatic_checks_enabledin(0,1))",
            "check(offered_versionisnullorlength(offered_version)between1and64)",
            "check(minimum_required_versionisnullorlength(minimum_required_version)between1and64)",
        ],
    ),
];

pub(super) const TABLE_COLUMNS: &[(&str, &[&str])] = &[
    (
        "claude_usage_daily",
        &[
            "day",
            "observed_tokens",
            "coverage",
            "observed_through",
            "revision",
            "priced_tokens",
            "cost_usd",
            "pricing_basis",
            "pricing_fingerprint",
        ],
    ),
    (
        "claude_usage_files",
        &[
            "path",
            "file_identity",
            "size_bytes",
            "modified_ns",
            "parsed_offset",
            "resume_anchor",
            "parser_version",
            "completion_state",
        ],
    ),
    (
        "claude_usage_frames",
        &["frame_key", "day", "observed_at", "parser_version"],
    ),
    ("claude_usage_index_meta", &["key", "value"]),
    (
        "claude_usage_message_supersedes",
        &[
            "replacement_frame_key",
            "superseded_frame_key",
            "parser_version",
        ],
    ),
    (
        "claude_usage_messages",
        &[
            "frame_key",
            "supersedes_frame_key",
            "message_key",
            "day",
            "observed_at",
            "model",
            "input_tokens",
            "cache_creation_input_tokens",
            "cache_read_input_tokens",
            "output_tokens",
            "cache_creation_5m_input_tokens",
            "cache_creation_1h_input_tokens",
            "service_tier",
            "inference_geo",
            "speed",
            "web_search_requests",
            "web_fetch_requests",
            "code_execution_requests",
            "has_unknown_paid_server_tool",
            "observed_tokens",
            "complete",
            "parser_version",
        ],
    ),
    ("codex_account_usage_days", &["day", "tokens"]),
    ("codex_account_usage_meta", &["singleton", "observed_at"]),
    (
        "codex_usage_file_days",
        &[
            "path",
            "day",
            "observed_tokens",
            "priced_tokens",
            "cost_usd",
            "complete",
            "observed_through",
            "priced_observed_through",
            "pricing_fingerprint",
        ],
    ),
    (
        "codex_usage_file_model_days",
        &[
            "path",
            "day",
            "model",
            "pricing_input_tokens",
            "input_tokens",
            "cached_input_tokens",
            "cache_write_input_tokens",
            "output_tokens",
            "reasoning_output_tokens",
            "observed_tokens",
            "cost_usd",
            "pricing_basis",
            "pricing_fingerprint",
            "complete",
            "observed_through",
        ],
    ),
    (
        "codex_usage_files",
        &[
            "path",
            "file_identity",
            "size_bytes",
            "modified_ns",
            "parsed_offset",
            "parsed_prefix_anchor",
            "parser_version",
            "completion_state",
            "deferred_until_day",
            "active_model",
            "baseline_is_inherited",
            "history_start_ordinal",
            "record_ordinal",
            "usage_excluded",
            "schema_supported",
            "previous_input",
            "previous_cached_input",
            "previous_cache_write_input",
            "previous_output",
            "previous_reasoning_output",
            "previous_total",
        ],
    ),
    ("codex_usage_index_meta", &["key", "value"]),
    (
        "lifecycle_state",
        &[
            "singleton",
            "bootstrap_completed",
            "profile_provisioning",
            "public_participation_authorized",
            "profile_retry_pending",
            "backfill_window_days",
            "display_name",
            "touch_grass_id",
            "recovery_disclosure_pending",
        ],
    ),
    ("provider_settings", &["provider", "enabled"]),
    (
        "sanitized_desktop_state",
        &[
            "singleton",
            "schema_version",
            "contract_version",
            "revision",
            "snapshot_json",
        ],
    ),
    ("touchgrassbar_schema_versions", &["module", "version"]),
    (
        "touchgrassbar_update_state",
        &[
            "singleton",
            "automatic_checks_enabled",
            "last_automatic_check_at",
            "offered_version",
            "minimum_required_version",
        ],
    ),
    (
        "touchgrassbar_update_state_v3",
        &[
            "singleton",
            "automatic_checks_enabled",
            "last_automatic_check_at",
            "offered_version",
            "minimum_required_version",
        ],
    ),
];

pub(super) fn normalize_sql(value: &str) -> String {
    #[derive(Clone, Copy)]
    enum Quote {
        None,
        Literal,
        DoubleQuoted,
    }

    let mut normalized = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    let mut quote = Quote::None;
    while let Some(character) = characters.next() {
        match quote {
            Quote::None => match character {
                '\'' => {
                    normalized.push(character);
                    quote = Quote::Literal;
                }
                '"' => {
                    normalized.push(character);
                    quote = Quote::DoubleQuoted;
                }
                character if character.is_ascii_whitespace() => {}
                character => normalized.push(character.to_ascii_lowercase()),
            },
            Quote::Literal => {
                normalized.push(character);
                if character == '\'' {
                    if characters.peek() == Some(&'\'') {
                        characters.next();
                        normalized.push('\'');
                    } else {
                        quote = Quote::None;
                    }
                }
            }
            Quote::DoubleQuoted => {
                normalized.push(character);
                if character == '"' {
                    if characters.peek() == Some(&'"') {
                        characters.next();
                        normalized.push('"');
                    } else {
                        quote = Quote::None;
                    }
                }
            }
        }
    }
    normalized
}
