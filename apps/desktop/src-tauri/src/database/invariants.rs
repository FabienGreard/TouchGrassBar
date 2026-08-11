use rusqlite::{Connection, params};

use crate::{providers, sanitized};

use super::{
    DatabaseOpenError,
    catalog::{
        COLUMN_DEFAULTS, DATABASE_FORMAT_VERSION, FOREIGN_KEYS, INDEX_DEFINITIONS, INDEXES,
        MODULES, NULLABLE_COLUMNS, PRIMARY_KEYS, STRICT_TABLES, TABLE_CHECKS, TABLE_COLUMNS,
        TABLES, VIEWS, normalize_sql,
    },
    inspection::{read_version_rows, reject_unknown_objects},
};

pub(super) fn verify_invariants(connection: &Connection) -> Result<(), DatabaseOpenError> {
    let integrity = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "integrity",
        })?;
    if integrity != "ok" {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "integrity",
        });
    }
    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "foreign-keys",
        })?;
    if foreign_keys
        .query([])
        .and_then(|mut rows| rows.next().map(|row| row.is_some()))
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "foreign-keys",
        })?
    {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "foreign-keys",
        });
    }

    let format_version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "database-format",
        })?;
    if format_version != DATABASE_FORMAT_VERSION {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "database-format",
        });
    }

    let mut expected_versions = MODULES
        .iter()
        .map(|(module, version)| ((*module).to_owned(), *version))
        .collect::<Vec<_>>();
    expected_versions.sort();
    if read_version_rows(connection)? != expected_versions {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "version-vector",
        });
    }
    reject_unknown_objects(connection).map_err(|_| DatabaseOpenError::InvariantFailed {
        invariant: "object-registry",
    })?;
    let mut stored_tables = object_names(connection, "table")?;
    stored_tables.sort();
    if stored_tables != TABLES {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "table-registry",
        });
    }
    let mut stored_indexes = object_names(connection, "index")?;
    stored_indexes.sort();
    if stored_indexes != INDEXES {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "index-registry",
        });
    }
    let mut stored_views = object_names(connection, "view")?;
    stored_views.sort();
    if stored_views != VIEWS {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "view-registry",
        });
    }
    verify_table_columns(connection)?;
    verify_table_definitions(connection)?;
    verify_foreign_key_definitions(connection)?;
    verify_index_definitions(connection)?;
    verify_view_definitions(connection)?;

    let lifecycle_profile = verify_lifecycle(connection)?;
    let sanitized_state = sanitized::read_database_state(connection).map_err(|_| {
        DatabaseOpenError::InvariantFailed {
            invariant: "sanitized-state",
        }
    })?;
    if sanitized_state.profile != lifecycle_profile {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "profile-projection",
        });
    }
    verify_update_state(connection)?;
    verify_usage_indexes(connection)?;
    Ok(())
}

fn object_names(
    connection: &Connection,
    object_type: &str,
) -> Result<Vec<&'static str>, DatabaseOpenError> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = ?1 AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "object-registry",
        })?;
    let names = statement
        .query_map([object_type], |row| row.get::<_, String>(0))
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "object-registry",
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "object-registry",
        })?;
    names
        .into_iter()
        .map(|name| {
            TABLES
                .iter()
                .chain(INDEXES.iter())
                .chain(VIEWS.iter())
                .copied()
                .find(|known| *known == name)
                .ok_or(DatabaseOpenError::InvariantFailed {
                    invariant: "object-registry",
                })
        })
        .collect()
}

fn verify_table_columns(connection: &Connection) -> Result<(), DatabaseOpenError> {
    for (table, expected) in TABLE_COLUMNS {
        let mut statement = connection
            .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-columns",
            })?;
        let mut columns = statement
            .query_map([table], |row| row.get::<_, String>(0))
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-columns",
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-columns",
            })?;
        let mut expected = expected
            .iter()
            .map(|column| (*column).to_owned())
            .collect::<Vec<_>>();
        columns.sort();
        expected.sort();
        if columns != expected {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "table-columns",
            });
        }
    }
    Ok(())
}

fn verify_table_definitions(connection: &Connection) -> Result<(), DatabaseOpenError> {
    for table in TABLES {
        let mut statement = connection
            .prepare(
                "SELECT name, \"notnull\", dflt_value, pk
                 FROM pragma_table_info(?1) ORDER BY cid",
            )
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            })?;
        let columns = statement
            .query_map([table], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            })?;
        let mut primary_key = columns
            .iter()
            .filter(|(_, _, _, position)| *position > 0)
            .map(|(name, _, _, position)| (*position, name.as_str()))
            .collect::<Vec<_>>();
        primary_key.sort_by_key(|(position, _)| *position);
        let expected_primary_key = PRIMARY_KEYS
            .iter()
            .find(|(known_table, _)| known_table == table)
            .map(|(_, columns)| *columns)
            .ok_or(DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            })?;
        if !primary_key
            .iter()
            .map(|(_, name)| *name)
            .eq(expected_primary_key.iter().copied())
        {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            });
        }

        let nullable = NULLABLE_COLUMNS
            .iter()
            .find(|(known_table, _)| known_table == table)
            .map(|(_, columns)| *columns)
            .unwrap_or_default();
        for (name, not_null, default, primary_key_position) in &columns {
            let is_nullable = !not_null && *primary_key_position == 0;
            if is_nullable != nullable.contains(&name.as_str()) {
                return Err(DatabaseOpenError::InvariantFailed {
                    invariant: "table-definitions",
                });
            }
            let expected_default = COLUMN_DEFAULTS
                .iter()
                .find(|(known_table, column, _)| known_table == table && column == name)
                .map(|(_, _, value)| normalize_sql(value));
            if default.as_deref().map(normalize_sql) != expected_default {
                return Err(DatabaseOpenError::InvariantFailed {
                    invariant: "table-definitions",
                });
            }
        }

        let schema = schema_sql(connection, "table", table, "table-definitions")?;
        if schema.ends_with(")strict") != STRICT_TABLES.contains(table) {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            });
        }
        let expected_checks = TABLE_CHECKS
            .iter()
            .find(|(known_table, _)| known_table == table)
            .map(|(_, checks)| *checks)
            .unwrap_or_default();
        let check_count = schema.matches("check(").count();
        let check_count_matches = if *table == "claude_usage_daily" {
            matches!(check_count, 4 | 5)
                && (check_count == 4
                    || schema.contains(
                        "check(correction_provenanceisnullorcorrection_provenance='parser-correction')",
                    ))
        } else {
            check_count == expected_checks.len()
        };
        if !check_count_matches || expected_checks.iter().any(|check| !schema.contains(check)) {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            });
        }
    }
    Ok(())
}

fn verify_foreign_key_definitions(connection: &Connection) -> Result<(), DatabaseOpenError> {
    for table in TABLES {
        let mut statement = connection
            .prepare(
                "SELECT \"from\", \"table\", \"to\", on_update, on_delete, match
                 FROM pragma_foreign_key_list(?1)",
            )
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "foreign-key-definitions",
            })?;
        let mut stored = statement
            .query_map([table], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "foreign-key-definitions",
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "foreign-key-definitions",
            })?;
        let mut expected = FOREIGN_KEYS
            .iter()
            .filter(|(known_table, _, _, _, _)| known_table == table)
            .map(|(_, from, target_table, target_column, on_delete)| {
                (
                    (*from).to_owned(),
                    (*target_table).to_owned(),
                    (*target_column).to_owned(),
                    "NO ACTION".to_owned(),
                    (*on_delete).to_owned(),
                    "NONE".to_owned(),
                )
            })
            .collect::<Vec<_>>();
        stored.sort();
        expected.sort();
        if stored != expected {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "foreign-key-definitions",
            });
        }
    }
    Ok(())
}

fn verify_index_definitions(connection: &Connection) -> Result<(), DatabaseOpenError> {
    for table in TABLES {
        let has_unique_constraint = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pragma_index_list(?1) WHERE origin = 'u'
                 )",
                [table],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            })?;
        if has_unique_constraint {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            });
        }
    }

    for (name, table, expected_columns, predicate) in INDEX_DEFINITIONS {
        let (unique, origin, partial) = connection
            .query_row(
                "SELECT \"unique\", origin, partial
                 FROM pragma_index_list(?1) WHERE name = ?2",
                params![table, name],
                |row| {
                    Ok((
                        row.get::<_, bool>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, bool>(2)?,
                    ))
                },
            )
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            })?;
        if unique || origin != "c" || partial != predicate.is_some() {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            });
        }
        let mut statement = connection
            .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            })?;
        let columns = statement
            .query_map([name], |row| row.get::<_, String>(0))
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            })?;
        if !columns
            .iter()
            .map(String::as_str)
            .eq(expected_columns.iter().copied())
        {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            });
        }

        let mut expected_sql = format!(
            "create index {name} on {table}({})",
            expected_columns.join(",")
        );
        if let Some(predicate) = predicate {
            expected_sql.push_str(" where ");
            expected_sql.push_str(predicate);
        }
        if schema_sql(connection, "index", name, "index-definitions")?
            != normalize_sql(&expected_sql)
        {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            });
        }
    }
    Ok(())
}

fn verify_view_definitions(connection: &Connection) -> Result<(), DatabaseOpenError> {
    let expected = normalize_sql(
        "CREATE VIEW touchgrassbar_update_state AS
         SELECT singleton, automatic_checks_enabled, last_automatic_check_at,
                offered_version, minimum_required_version
         FROM touchgrassbar_update_state_v3",
    );
    if schema_sql(
        connection,
        "view",
        "touchgrassbar_update_state",
        "view-definitions",
    )? != expected
    {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "view-definitions",
        });
    }
    Ok(())
}

fn schema_sql(
    connection: &Connection,
    object_type: &str,
    name: &str,
    invariant: &'static str,
) -> Result<String, DatabaseOpenError> {
    connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = ?1 AND name = ?2",
            params![object_type, name],
            |row| row.get::<_, String>(0),
        )
        .map(|sql| normalize_sql(&sql))
        .map_err(|_| DatabaseOpenError::InvariantFailed { invariant })
}

fn verify_lifecycle(
    connection: &Connection,
) -> Result<sanitized::SanitizedProfileOutcome, DatabaseOpenError> {
    let invalid_lifecycle = connection
        .query_row(
            "SELECT COUNT(*) != 1 OR EXISTS(
               SELECT 1 FROM lifecycle_state
               WHERE singleton != 1
                  OR bootstrap_completed NOT IN (0, 1)
                  OR profile_provisioning NOT IN ('not-authorized', 'profile-pending', 'ready')
                  OR public_participation_authorized NOT IN (0, 1)
                  OR profile_retry_pending NOT IN (0, 1)
                  OR recovery_disclosure_pending NOT IN (0, 1)
                  OR (backfill_window_days IS NOT NULL AND backfill_window_days != 30)
                  OR (display_name IS NOT NULL AND length(trim(display_name)) NOT BETWEEN 1 AND 40)
                  OR NOT (
                    (
                      bootstrap_completed = 0
                      AND profile_provisioning = 'not-authorized'
                      AND public_participation_authorized = 0
                      AND profile_retry_pending = 0
                      AND backfill_window_days IS NULL
                      AND display_name IS NULL
                      AND touch_grass_id IS NULL
                      AND recovery_disclosure_pending = 0
                    ) OR (
                      bootstrap_completed = 1
                      AND profile_provisioning = 'profile-pending'
                      AND public_participation_authorized = 1
                      AND profile_retry_pending = 1
                      AND backfill_window_days = 30
                      AND display_name IS NOT NULL
                      AND touch_grass_id IS NULL
                      AND recovery_disclosure_pending = 0
                    ) OR (
                      bootstrap_completed = 1
                      AND profile_provisioning = 'ready'
                      AND public_participation_authorized = 1
                      AND profile_retry_pending = 0
                      AND backfill_window_days = 30
                      AND display_name IS NOT NULL
                      AND touch_grass_id IS NOT NULL
                    )
                  )
             ) FROM lifecycle_state",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "lifecycle-state",
        })?;
    if invalid_lifecycle {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "lifecycle-state",
        });
    }
    let valid_settings = connection
        .query_row(
            "SELECT COUNT(*) = 2
               AND SUM(provider = 'codex') = 1
               AND SUM(provider = 'claude') = 1
               AND SUM(enabled NOT IN (0, 1)) = 0
             FROM provider_settings",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "provider-settings",
        })?;
    if !valid_settings {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "provider-settings",
        });
    }
    let (status, display_name, touch_grass_id) = connection
        .query_row(
            "SELECT profile_provisioning, display_name, touch_grass_id
             FROM lifecycle_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "lifecycle-state",
        })?;
    match (status.as_str(), display_name, touch_grass_id) {
        ("not-authorized", None, None) => Ok(sanitized::SanitizedProfileOutcome::NotAuthorized),
        ("profile-pending", Some(_), None) => {
            Ok(sanitized::SanitizedProfileOutcome::ProfilePending)
        }
        ("ready", Some(display_name), Some(touch_grass_id)) => {
            Ok(sanitized::SanitizedProfileOutcome::Ready {
                display_name,
                touch_grass_id,
            })
        }
        _ => Err(DatabaseOpenError::InvariantFailed {
            invariant: "lifecycle-state",
        }),
    }
}

fn verify_update_state(connection: &Connection) -> Result<(), DatabaseOpenError> {
    let valid = connection
        .query_row(
            "SELECT COUNT(*) = 1 AND SUM(
               singleton = 1
               AND automatic_checks_enabled IN (0, 1)
               AND (offered_version IS NULL OR length(offered_version) BETWEEN 1 AND 64)
               AND (
                 minimum_required_version IS NULL OR
                 length(minimum_required_version) BETWEEN 1 AND 64
               )
             ) = 1
             FROM touchgrassbar_update_state_v3",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        })?;
    if !valid {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        });
    }
    let (last_check, offered, minimum) = connection
        .query_row(
            "SELECT last_automatic_check_at, offered_version, minimum_required_version
             FROM touchgrassbar_update_state_v3 WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        })?;
    if last_check.is_some_and(|timestamp| timestamp < 0) {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        });
    }
    let offered = offered
        .as_deref()
        .map(semver::Version::parse)
        .transpose()
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        })?;
    let minimum = minimum
        .as_deref()
        .map(semver::Version::parse)
        .transpose()
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        })?;
    if offered
        .as_ref()
        .zip(minimum.as_ref())
        .is_some_and(|(offered, minimum)| offered < minimum)
    {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        });
    }
    Ok(())
}

fn verify_usage_indexes(connection: &Connection) -> Result<(), DatabaseOpenError> {
    if providers::codex_usage_schema_version(connection).map_err(|_| {
        DatabaseOpenError::InvariantFailed {
            invariant: "codex-usage-index",
        }
    })? != providers::CODEX_USAGE_SCHEMA_VERSION
    {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "codex-usage-index",
        });
    }
    if providers::claude_usage_schema_version(connection).map_err(|_| {
        DatabaseOpenError::InvariantFailed {
            invariant: "claude-usage-index",
        }
    })? != providers::CLAUDE_USAGE_SCHEMA_VERSION
    {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "claude-usage-index",
        });
    }
    for (invariant, query) in [
        (
            "codex-usage-values",
            "SELECT EXISTS(
               SELECT 1 FROM codex_account_usage_days
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10 OR tokens < 0
             ) OR EXISTS(
               SELECT 1 FROM codex_usage_file_model_days
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10
                  OR pricing_input_tokens < 0 OR input_tokens < 0
                  OR cached_input_tokens < 0 OR cache_write_input_tokens < 0
                  OR output_tokens < 0 OR reasoning_output_tokens < 0
                  OR observed_tokens < 0 OR complete NOT IN (0, 1)
                  OR pricing_mode NOT IN ('standard', 'fast')
             ) OR EXISTS(
               SELECT 1 FROM codex_usage_file_days
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10
                  OR observed_tokens < 0 OR priced_tokens < 0 OR cost_usd < 0
                  OR complete NOT IN (0, 1)
             ) OR EXISTS(
               SELECT 1 FROM codex_usage_files
               WHERE (deferred_until_day IS NOT NULL AND (
                        strftime('%Y-%m-%d', deferred_until_day, '+0 days') IS NULL
                        OR strftime('%Y-%m-%d', deferred_until_day, '+0 days')
                           != deferred_until_day
                        OR length(deferred_until_day) != 10
                      ))
                  OR lineage_mode NOT IN (
                    'unknown', 'root', 'discovering', 'explicit-boundary',
                    'independent', 'parent-resolved', 'unresolved'
                  )
                  OR usage_excluded NOT IN (0, 1)
                  OR schema_supported NOT IN (0, 1)
                  OR parent_identity_explicit NOT IN (0, 1)
                  OR embedded_ancestor_seen NOT IN (0, 1)
                  OR lineage_invalid NOT IN (0, 1)
                  OR last_turn_context_is_first NOT IN (0, 1)
                  OR marker_based_boundary NOT IN (0, 1)
                  OR marker_candidate_invalidated NOT IN (0, 1)
                  OR (marker_local_confirmation IS NOT NULL
                      AND marker_local_confirmation NOT IN (0, 1))
                  OR accounting_ready NOT IN (0, 1)
                  OR parser_error_seen NOT IN (0, 1)
                  OR snapshot_timestamp_regressed NOT IN (0, 1)
             ) OR EXISTS(
               SELECT 1 FROM codex_usage_token_snapshots
               WHERE record_ordinal < 0 OR timestamp_ns < 0
                  OR input_tokens < 0 OR cached_input_tokens < 0
                  OR cache_write_input_tokens < 0 OR output_tokens < 0
                  OR reasoning_output_tokens < 0 OR total_tokens < 0
             )",
        ),
        (
            "claude-usage-values",
            "SELECT EXISTS(
               SELECT 1 FROM claude_usage_messages
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10
                  OR input_tokens < 0 OR cache_creation_input_tokens < 0
                  OR cache_read_input_tokens < 0 OR output_tokens < 0
                  OR observed_tokens < 0 OR has_unknown_paid_server_tool NOT IN (0, 1)
                  OR complete NOT IN (0, 1)
             ) OR EXISTS(
               SELECT 1 FROM claude_usage_frames
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10
             ) OR EXISTS(
               SELECT 1 FROM claude_usage_daily
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10
                  OR observed_tokens < 0 OR priced_tokens < 0
                  OR cost_usd < 0 OR coverage NOT IN ('complete', 'partial')
                  OR revision < 1 OR cost_modeled NOT IN (0, 1)
                  OR (correction_provenance IS NULL)
                     != (correction_source_revision IS NULL)
                  OR (correction_provenance IS NOT NULL AND (
                        correction_provenance != 'parser-correction'
                        OR correction_source_revision < 1
                        OR correction_source_revision > revision
                     ))
             )",
        ),
        (
            "claude-correction-edges",
            "SELECT EXISTS(
               SELECT 1
               FROM claude_usage_message_supersedes AS edge
               LEFT JOIN claude_usage_frames AS replacement
                 ON replacement.frame_key = edge.replacement_frame_key
               LEFT JOIN claude_usage_messages AS superseded
                 ON superseded.frame_key = edge.superseded_frame_key
               LEFT JOIN claude_usage_frames AS superseded_frame
                 ON superseded_frame.frame_key = edge.superseded_frame_key
               WHERE edge.replacement_frame_key = edge.superseded_frame_key
                  OR replacement.frame_key IS NULL
                  OR superseded.frame_key IS NULL
                  OR superseded_frame.frame_key IS NULL
                  OR edge.parser_version != replacement.parser_version
                  OR edge.parser_version != superseded.parser_version
                  OR edge.parser_version != superseded_frame.parser_version
                  OR edge.aggregate_applied NOT IN (0, 1)
             ) OR EXISTS(
               SELECT 1
               FROM claude_usage_messages AS message
               LEFT JOIN claude_usage_frames AS frame
                 ON frame.frame_key = message.frame_key
               WHERE frame.frame_key IS NULL
                  OR frame.day != message.day
                  OR frame.observed_at != message.observed_at
                  OR frame.parser_version != message.parser_version
             )",
        ),
        (
            "usage-sync-values",
            "SELECT (
               SELECT COUNT(*) > 1 FROM usage_sync_generations
               WHERE queue_state IN ('active', 'blocked')
             )",
        ),
        (
            "usage-retention",
            "SELECT
               COALESCE((
                 SELECT julianday(MAX(day)) - julianday(MIN(day)) > 29
                 FROM codex_usage_file_days
               ), 0)
               OR COALESCE((
                 SELECT julianday(MAX(day)) - julianday(MIN(day)) > 29
                 FROM codex_usage_file_model_days
               ), 0)
               OR COALESCE((
                 SELECT julianday(MAX(day)) - julianday(MIN(day)) > 59
                 FROM claude_usage_daily
               ), 0)
               OR COALESCE((
                 SELECT julianday(MAX(day)) - julianday(MIN(day)) > 59
                 FROM claude_usage_messages
               ), 0)
               OR COALESCE((
                 SELECT julianday(MAX(day)) - julianday(MIN(day)) > 59
                 FROM claude_usage_frames
               ), 0)",
        ),
    ] {
        let invalid = connection
            .query_row(query, [], |row| row.get::<_, bool>(0))
            .map_err(|_| DatabaseOpenError::InvariantFailed { invariant })?;
        if invalid {
            return Err(DatabaseOpenError::InvariantFailed { invariant });
        }
    }
    Ok(())
}
