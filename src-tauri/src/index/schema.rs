use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior};

use super::error::{IndexError, IndexResult};

pub(crate) const SCHEMA_VERSION: i64 = 3;
pub(crate) const PARSER_SEMANTICS_VERSION: i64 = 2;
// ASCII "CUXI" (Codex Usage indeX Index).
pub(crate) const APPLICATION_ID: i64 = 0x4355_5849;

pub(crate) fn configure(connection: &Connection) -> IndexResult<()> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;

    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(IndexError::InvalidState("SQLite refused WAL mode"));
    }

    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "wal_autocheckpoint", 1_000_i64)?;
    Ok(())
}

pub(crate) fn migrate(connection: &mut Connection) -> IndexResult<()> {
    let application_id: i64 =
        connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    if application_id != 0 && application_id != APPLICATION_ID {
        return Err(IndexError::ForeignDatabase(application_id));
    }

    let current_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current_version > SCHEMA_VERSION {
        return Err(IndexError::UnsupportedSchema(current_version));
    }
    if current_version == SCHEMA_VERSION {
        verify_parser_version(connection)?;
        return Ok(());
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if current_version == 0 {
        transaction.execute_batch(V1_SCHEMA)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at_ms)
             VALUES (1, 'initial incremental rollout index', 'v1', unixepoch('subsec') * 1000)",
            [],
        )?;
        transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
        transaction.pragma_update(None, "user_version", 1_i64)?;
    }
    if current_version <= 1 {
        // A missing original log must never erase historical usage. Old rows
        // keep their token values; cache availability remains unknown until a
        // successful reread can enrich them.
        transaction.execute_batch(
            "ALTER TABLE usage_events ADD COLUMN cache_metrics_available INTEGER NOT NULL DEFAULT 0 CHECK(cache_metrics_available IN (0, 1));
             UPDATE rollout_files SET parser_version = 0;",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at_ms)
             VALUES (2, 'cache availability without historical data loss', 'v2-preserving', unixepoch('subsec') * 1000)",
            [],
        )?;
        transaction.execute(
            "INSERT INTO app_meta(key, value) VALUES ('parser_semantics_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [PARSER_SEMANTICS_VERSION.to_string()],
        )?;
    }
    if current_version <= 2 {
        transaction.execute_batch(
            "ALTER TABLE rollout_files ADD COLUMN source_stamp TEXT;
             CREATE TABLE session_snapshots (
               rollout_key_hash BLOB PRIMARY KEY CHECK(length(rollout_key_hash) = 32),
               file_id INTEGER UNIQUE REFERENCES rollout_files(id)
             ) WITHOUT ROWID;
             INSERT INTO session_snapshots(rollout_key_hash, file_id)
             SELECT DISTINCT rollout_key_hash, NULL FROM rollout_files;",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, name, checksum, applied_at_ms)
             VALUES (3, 'permanent session history and compressed sources', 'v3', unixepoch('subsec') * 1000)",
            [],
        )?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    transaction.commit()?;
    verify_parser_version(connection)
}

pub(crate) fn verify_parser_version(connection: &Connection) -> IndexResult<()> {
    let persisted: Option<String> = connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = 'parser_semantics_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let parsed = persisted
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok());
    if parsed != Some(PARSER_SEMANTICS_VERSION) {
        return Err(IndexError::InvalidState(
            "parser semantics changed; derived index rebuild is required",
        ));
    }
    Ok(())
}

use rusqlite::OptionalExtension;

const V1_SCHEMA: &str = r#"
CREATE TABLE schema_migrations (
  version       INTEGER PRIMARY KEY,
  name          TEXT NOT NULL,
  checksum      TEXT NOT NULL,
  applied_at_ms INTEGER NOT NULL
);

CREATE TABLE app_meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE models (
  id    INTEGER PRIMARY KEY,
  key   TEXT NOT NULL UNIQUE,
  label TEXT NOT NULL
);

CREATE TABLE projects (
  id    INTEGER PRIMARY KEY,
  key   BLOB NOT NULL UNIQUE CHECK(length(key) = 32),
  label TEXT NOT NULL
);

CREATE TABLE rollout_files (
  id                  INTEGER PRIMARY KEY,
  path_key            BLOB NOT NULL UNIQUE CHECK(length(path_key) = 32),
  rollout_key_hash    BLOB NOT NULL CHECK(length(rollout_key_hash) = 32),
  generation          INTEGER NOT NULL DEFAULT 1 CHECK(generation > 0),
  identity_kind       TEXT NOT NULL DEFAULT 'path-fallback'
                        CHECK(identity_kind IN ('path-fallback', 'windows-file-id')),
  volume_serial       BLOB,
  file_id_128         BLOB,
  creation_ms         INTEGER,
  size_bytes          INTEGER NOT NULL DEFAULT 0 CHECK(size_bytes >= 0),
  mtime_ms             INTEGER,
  committed_offset    INTEGER NOT NULL DEFAULT 0 CHECK(committed_offset >= 0),
  committed_lines     INTEGER NOT NULL DEFAULT 0 CHECK(committed_lines >= 0),
  prefix_len          INTEGER NOT NULL DEFAULT 0 CHECK(prefix_len BETWEEN 0 AND 4096),
  prefix_hash         BLOB,
  guard_offset        INTEGER NOT NULL DEFAULT 0 CHECK(guard_offset >= 0),
  guard_len           INTEGER NOT NULL DEFAULT 0 CHECK(guard_len BETWEEN 0 AND 4096),
  guard_hash          BLOB,
  incomplete_tail     INTEGER NOT NULL DEFAULT 0 CHECK(incomplete_tail IN (0, 1)),
  present             INTEGER NOT NULL DEFAULT 1 CHECK(present IN (0, 1)),
  parser_version      INTEGER NOT NULL,
  updated_at_ms       INTEGER NOT NULL
);

CREATE TABLE rollout_parser_state (
  file_id                         INTEGER PRIMARY KEY
                                    REFERENCES rollout_files(id) ON DELETE CASCADE,
  first_record_seen               INTEGER NOT NULL CHECK(first_record_seen IN (0, 1)),
  next_history_ordinal            INTEGER NOT NULL CHECK(next_history_ordinal >= 0),
  is_subagent                     INTEGER NOT NULL CHECK(is_subagent IN (0, 1)),
  ownership_mode                  TEXT NOT NULL
                                    CHECK(ownership_mode IN ('all', 'ordinal', 'legacy-marker', 'unresolved')),
  boundary_ordinal                INTEGER CHECK(boundary_ordinal >= 0),
  owned                           INTEGER NOT NULL CHECK(owned IN (0, 1)),
  current_model_id                INTEGER NOT NULL REFERENCES models(id),
  current_project_id              INTEGER NOT NULL REFERENCES projects(id),
  legacy_model_id                 INTEGER REFERENCES models(id),
  legacy_project_id               INTEGER REFERENCES projects(id),
  previous_input_tokens           INTEGER CHECK(previous_input_tokens >= 0),
  previous_cached_input_tokens    INTEGER CHECK(previous_cached_input_tokens >= 0),
  previous_output_tokens          INTEGER CHECK(previous_output_tokens >= 0),
  previous_reasoning_output_tokens INTEGER CHECK(previous_reasoning_output_tokens >= 0),
  previous_total_tokens           INTEGER CHECK(previous_total_tokens >= 0),
  last_fallback_pending           INTEGER NOT NULL CHECK(last_fallback_pending IN (0, 1)),
  skipped_events                  INTEGER NOT NULL DEFAULT 0 CHECK(skipped_events >= 0),
  filtered_parent_events          INTEGER NOT NULL DEFAULT 0 CHECK(filtered_parent_events >= 0)
);

CREATE TABLE usage_events (
  id                       INTEGER PRIMARY KEY,
  file_id                  INTEGER NOT NULL REFERENCES rollout_files(id) ON DELETE CASCADE,
  logical_event_key        BLOB NOT NULL CHECK(length(logical_event_key) = 32),
  source_ordinal           INTEGER CHECK(source_ordinal >= 0),
  source_line              INTEGER NOT NULL CHECK(source_line > 0),
  end_offset               INTEGER NOT NULL CHECK(end_offset > 0),
  occurred_at_ms           INTEGER,
  source_kind              TEXT NOT NULL
                             CHECK(source_kind IN ('total-delta', 'total-segment-start', 'last-fallback')),
  model_id                 INTEGER NOT NULL REFERENCES models(id),
  project_id               INTEGER NOT NULL REFERENCES projects(id),
  input_tokens             INTEGER NOT NULL CHECK(input_tokens >= 0),
  cached_input_tokens      INTEGER NOT NULL CHECK(cached_input_tokens >= 0),
  output_tokens            INTEGER NOT NULL CHECK(output_tokens >= 0),
  reasoning_output_tokens  INTEGER NOT NULL CHECK(reasoning_output_tokens >= 0),
  total_tokens             INTEGER NOT NULL CHECK(total_tokens >= 0),
  payload_hash             BLOB NOT NULL CHECK(length(payload_hash) = 32),
  UNIQUE(file_id, end_offset)
);

CREATE INDEX usage_events_logical_key ON usage_events(logical_event_key);
CREATE INDEX usage_events_timestamp ON usage_events(occurred_at_ms);
CREATE INDEX usage_events_model ON usage_events(model_id);
CREATE INDEX usage_events_project ON usage_events(project_id);

CREATE TABLE file_warning_counts (
  file_id   INTEGER NOT NULL REFERENCES rollout_files(id) ON DELETE CASCADE,
  code      TEXT NOT NULL,
  count     INTEGER NOT NULL CHECK(count > 0),
  last_line INTEGER CHECK(last_line > 0),
  PRIMARY KEY(file_id, code)
) WITHOUT ROWID;
"#;
