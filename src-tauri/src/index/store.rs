use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Local, NaiveDate, SecondsFormat, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::error::{IndexError, IndexResult};
use super::model::{
    Attribution, FileScanReport, LocalUsageEvent, LocalUsageSummary, OwnershipMode, ParserState,
    PendingUsageEvent, ScanSummary, TokenUsage, UsageDimension,
};
use super::parser::{hash_bytes, initial_state, process_complete_line, MAX_LINE_BYTES};
use super::schema::{configure, migrate, PARSER_SEMANTICS_VERSION};

const HASH_WINDOW_BYTES: usize = 4 * 1024;

pub struct IncrementalIndex {
    connection: Connection,
}

#[derive(Debug)]
struct FileRow {
    id: i64,
    source_stamp: Option<String>,
    parser_version: i64,
    creation_ms: Option<i64>,
    committed_offset: i64,
    committed_lines: i64,
    prefix_len: i64,
    prefix_hash: Option<Vec<u8>>,
    guard_offset: i64,
    guard_len: i64,
    guard_hash: Option<Vec<u8>>,
    incomplete_tail: bool,
}

#[derive(Debug)]
struct ReadResult {
    state: ParserState,
    events: Vec<PendingUsageEvent>,
    warnings: BTreeMap<&'static str, (i64, i64)>,
    committed_offset: i64,
    complete_lines_added: i64,
    incomplete_tail: bool,
}

impl IncrementalIndex {
    /// Reads an existing index without creating, configuring or migrating it.
    pub(crate) fn read_existing_events(
        database_path: &Path,
        range: Option<(i64, i64)>,
    ) -> IndexResult<Vec<LocalUsageEvent>> {
        let connection =
            Connection::open_with_flags(database_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let application_id: i64 =
            connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
        if application_id != super::schema::APPLICATION_ID {
            return Err(IndexError::ForeignDatabase(application_id));
        }
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version != super::schema::SCHEMA_VERSION {
            return Err(IndexError::UnsupportedSchema(version));
        }
        super::schema::verify_parser_version(&connection)?;
        query_local_events(&connection, range)
    }

    pub fn open(database_path: impl AsRef<Path>) -> IndexResult<Self> {
        if let Some(parent) = database_path.as_ref().parent() {
            fs::create_dir_all(parent)?;
        }
        let database_path = database_path.as_ref();
        let mut connection = Connection::open(database_path)?;
        backup_before_migration(&connection, database_path)?;
        configure(&connection)?;
        migrate(&mut connection)?;
        Ok(Self { connection })
    }

    pub fn open_in_memory() -> IndexResult<Self> {
        let mut connection = Connection::open_in_memory()?;
        // In-memory databases report "memory" rather than "wal". Tests still
        // exercise the same schema and transactions, so configure only the
        // pragmas that are meaningful for this connection type.
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut connection)?;
        Ok(Self { connection })
    }

    /// Scans active and archived, plain and compressed rollout files. Discovery must
    /// finish successfully before unseen database rows are marked absent.
    pub fn scan_codex_home(&mut self, codex_home: impl AsRef<Path>) -> IndexResult<ScanSummary> {
        self.scan_codex_home_with_progress(codex_home, |_, _| {})
    }

    pub fn scan_codex_home_with_progress<F>(
        &mut self,
        codex_home: impl AsRef<Path>,
        mut on_progress: F,
    ) -> IndexResult<ScanSummary>
    where
        F: FnMut(usize, usize),
    {
        let mut files = Vec::new();
        discover_rollouts(&codex_home.as_ref().join("sessions"), &mut files)?;
        discover_rollouts(&codex_home.as_ref().join("archived_sessions"), &mut files)?;
        files.sort();
        on_progress(0, files.len());

        let mut summary = ScanSummary {
            discovered_files: files.len(),
            ..ScanSummary::default()
        };
        let mut visible_keys = Vec::with_capacity(files.len());
        let mut failed_compressed_files = 0_i64;
        let mut failed_plain_files = 0_i64;
        for (index, path) in files.iter().enumerate() {
            visible_keys.push(path_key(path)?);
            let report = match self.scan_file(path) {
                Ok(report) => report,
                Err(error) if matches!(&error, IndexError::Io(_) | IndexError::InvalidState(_)) => {
                    // A truncated frame may already have emitted valid JSONL.
                    // scan_file commits nothing until the decoder reaches EOF.
                    if is_compressed(path) {
                        failed_compressed_files += 1;
                    } else {
                        failed_plain_files += 1;
                    }
                    on_progress(index + 1, files.len());
                    continue;
                }
                Err(error) => return Err(error),
            };
            if report.complete_lines_added > 0 || report.reset {
                summary.changed_files += 1;
            }
            summary.indexed_events_added += report.indexed_events_added;
            if report.incomplete_tail {
                summary.incomplete_files += 1;
            }
            on_progress(index + 1, files.len());
        }

        // Reconciliation is deliberately after all scans. A traversal/read
        // failure returns before this transaction, preserving prior presence.
        let transaction = self.connection.transaction()?;
        transaction.execute("UPDATE rollout_files SET present = 0", [])?;
        for key in visible_keys {
            transaction.execute(
                "UPDATE rollout_files SET present = 1 WHERE path_key = ?1",
                [key],
            )?;
        }
        transaction.execute(
            "INSERT INTO app_meta(key, value) VALUES ('compressed-read-failures', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [failed_compressed_files.to_string()],
        )?;
        transaction.execute(
            "INSERT INTO app_meta(key, value) VALUES ('plain-read-failures', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [failed_plain_files.to_string()],
        )?;
        transaction.commit()?;
        Ok(summary)
    }

    pub fn scan_file(&mut self, path: impl AsRef<Path>) -> IndexResult<FileScanReport> {
        let path = path.as_ref();
        let path_key = path_key(path)?;
        let rollout_key_hash = rollout_key_hash(path)?;
        let metadata = fs::metadata(path)?;
        let current_size = i64::try_from(metadata.len())
            .map_err(|_| IndexError::InvalidState("rollout file exceeds SQLite integer range"))?;
        let creation_ms = metadata.created().ok().and_then(system_time_ms);
        let mtime_ms = metadata.modified().ok().and_then(system_time_ms);
        let stamp = source_stamp(&metadata);
        let compressed = is_compressed(path);

        let file_row = self.ensure_file(
            &path_key,
            &rollout_key_hash,
            creation_ms,
            current_size,
            mtime_ms,
        )?;
        let source_changed = file_row.source_stamp.as_ref() != Some(&stamp);
        if compressed
            && file_row.source_stamp.as_ref() == Some(&stamp)
            && file_row.parser_version == PARSER_SEMANTICS_VERSION
        {
            self.connection.execute(
                "UPDATE rollout_files SET present = 1 WHERE id = ?1",
                [file_row.id],
            )?;
            return Ok(FileScanReport {
                file_id: file_row.id,
                complete_lines_added: 0,
                indexed_events_added: 0,
                committed_offset: 0,
                incomplete_tail: file_row.incomplete_tail,
                reset: false,
            });
        }
        let reset =
            compressed || self.requires_reset(path, &file_row, creation_ms, current_size)?;
        let mut state = if reset {
            initial_state()
        } else {
            self.load_state(file_row.id)?
        };
        let start_offset = if reset { 0 } else { file_row.committed_offset };
        let start_lines = if reset { 0 } else { file_row.committed_lines };
        let read = if compressed {
            let decoder = zstd::stream::read::Decoder::new(File::open(path)?)?;
            read_stream(
                BufReader::with_capacity(64 * 1024, decoder),
                0,
                0,
                &mut state,
                &rollout_key_hash,
            )?
        } else {
            read_increment(
                path,
                start_offset,
                start_lines,
                &mut state,
                &rollout_key_hash,
            )?
        };

        let final_metadata = fs::metadata(path)?;
        let final_size = i64::try_from(final_metadata.len())
            .map_err(|_| IndexError::InvalidState("rollout file exceeds SQLite integer range"))?;
        let final_creation_ms = final_metadata.created().ok().and_then(system_time_ms);
        let final_mtime_ms = final_metadata.modified().ok().and_then(system_time_ms);
        if compressed && source_stamp(&final_metadata) != stamp {
            return Err(IndexError::InvalidState(
                "compressed source changed while reading",
            ));
        }
        let incomplete_tail =
            read.incomplete_tail || (!compressed && final_size > read.committed_offset);
        let stored_offset = if compressed { 0 } else { read.committed_offset };
        let prefix_len = stored_offset.min(HASH_WINDOW_BYTES as i64);
        let prefix_hash = hash_segment(path, 0, prefix_len)?;
        let guard_len = stored_offset.min(HASH_WINDOW_BYTES as i64);
        let guard_offset = stored_offset - guard_len;
        let guard_hash = hash_segment(path, guard_offset, guard_len)?;

        let transaction = self.connection.transaction()?;
        // Any full read can replace the session snapshot, including a newly
        // discovered copy. Protect known history across all paths/formats;
        // normal active-file increments may still commit complete new lines.
        let has_known_history = start_offset == 0
            && (file_row.committed_lines > 0
                || transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM usage_events e
                   JOIN rollout_files f ON f.id = e.file_id
                   WHERE f.rollout_key_hash = ?1)",
                    [&rollout_key_hash],
                    |row| row.get::<_, bool>(0),
                )?);
        if has_known_history
            && (!read.state.first_record_seen
                || incomplete_tail
                || read.warnings.keys().any(|code| {
                    matches!(
                        *code,
                        "invalid-json" | "invalid-session-meta" | "invalid-token-count"
                    )
                }))
        {
            return Err(IndexError::InvalidState(
                "replacement source is incomplete or malformed",
            ));
        }
        if reset {
            transaction.execute("DELETE FROM usage_events WHERE file_id = ?1", [file_row.id])?;
            transaction.execute(
                "DELETE FROM file_warning_counts WHERE file_id = ?1",
                [file_row.id],
            )?;
            transaction.execute(
                "DELETE FROM rollout_parser_state WHERE file_id = ?1",
                [file_row.id],
            )?;
        }

        let mut inserted_events = 0_i64;
        for event in &read.events {
            inserted_events += insert_event(&transaction, file_row.id, event)?;
        }
        for (code, (count, last_line)) in &read.warnings {
            transaction.execute(
                "INSERT INTO file_warning_counts(file_id, code, count, last_line)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(file_id, code) DO UPDATE SET
                   count = count + excluded.count,
                   last_line = excluded.last_line",
                params![file_row.id, code, count, last_line],
            )?;
        }
        save_state(&transaction, file_row.id, &read.state)?;
        transaction.execute(
            "UPDATE rollout_files SET
               rollout_key_hash = ?2,
               generation = generation + ?3,
               creation_ms = ?4,
               size_bytes = ?5,
               mtime_ms = ?6,
               committed_offset = ?7,
               committed_lines = ?8,
               prefix_len = ?9,
               prefix_hash = ?10,
               guard_offset = ?11,
               guard_len = ?12,
               guard_hash = ?13,
               incomplete_tail = ?14,
               present = 1,
               parser_version = ?15,
               updated_at_ms = ?16,
               source_stamp = ?17
             WHERE id = ?1",
            params![
                file_row.id,
                rollout_key_hash,
                i64::from(reset),
                final_creation_ms,
                final_size,
                final_mtime_ms,
                stored_offset,
                start_lines + read.complete_lines_added,
                prefix_len,
                prefix_hash,
                guard_offset,
                guard_len,
                guard_hash,
                i64::from(incomplete_tail),
                PARSER_SEMANTICS_VERSION,
                Utc::now().timestamp_millis(),
                stamp,
            ],
        )?;
        // Keep one complete snapshot per session, including after its source
        // vanishes. A newly found short copy cannot hide a longer history.
        // Rereading the selected file can deliberately correct that history.
        transaction.execute(
            "INSERT INTO session_snapshots(rollout_key_hash, file_id) VALUES (?1, ?2)
             ON CONFLICT(rollout_key_hash) DO UPDATE SET file_id = excluded.file_id
             WHERE session_snapshots.file_id = excluded.file_id
                OR (session_snapshots.file_id IS NULL AND ?5 AND (
                     ?6 OR (SELECT committed_lines FROM rollout_files WHERE id = excluded.file_id)
                       >= (SELECT max(committed_lines) FROM rollout_files WHERE rollout_key_hash = ?1)))
                OR ?4
                OR (SELECT committed_lines FROM rollout_files WHERE id = excluded.file_id)
                   >= (SELECT committed_lines FROM rollout_files WHERE id = session_snapshots.file_id)
                   AND ?3",
            params![rollout_key_hash, file_row.id, i64::from(source_changed || (!reset && read.complete_lines_added > 0)),
                i64::from(reset && file_row.committed_lines > 0
                    && source_changed && file_row.parser_version == PARSER_SEMANTICS_VERSION),
                i64::from(start_offset == 0 && !incomplete_tail),
                i64::from(file_row.committed_lines > 0)],
        )?;
        transaction.commit()?;

        Ok(FileScanReport {
            file_id: file_row.id,
            complete_lines_added: read.complete_lines_added,
            indexed_events_added: inserted_events,
            committed_offset: stored_offset,
            incomplete_tail,
            reset,
        })
    }

    /// Rebuild by rereading available sources. Historical events remain intact
    /// until each replacement has been read successfully and committed.
    pub(crate) fn invalidate_scan_state_for_rebuild(&mut self) -> IndexResult<()> {
        self.connection
            .execute("UPDATE rollout_files SET parser_version = 0", [])?;
        Ok(())
    }

    pub fn aggregate(&self) -> IndexResult<LocalUsageSummary> {
        let now = Local::now();
        let local_date = now.date_naive();
        let start_naive = local_date
            .and_hms_opt(0, 0, 0)
            .ok_or(IndexError::InvalidState("invalid local date"))?;
        let end_naive = local_date
            .succ_opt()
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .ok_or(IndexError::InvalidState("invalid next local date"))?;
        let start = Local
            .from_local_datetime(&start_naive)
            .earliest()
            .ok_or(IndexError::InvalidState("local midnight does not exist"))?;
        let end =
            Local
                .from_local_datetime(&end_naive)
                .latest()
                .ok_or(IndexError::InvalidState(
                    "next local midnight does not exist",
                ))?;
        self.aggregate_for_range(start.timestamp_millis(), end.timestamp_millis())
    }

    pub fn aggregate_for_range(
        &self,
        today_start_ms: i64,
        today_end_ms: i64,
    ) -> IndexResult<LocalUsageSummary> {
        let total = query_usage(&self.connection, None)?;
        let today = query_usage(&self.connection, Some((today_start_ms, today_end_ms)))?;
        let by_model = query_model_dimensions(&self.connection)?;
        let by_project = query_project_dimensions(&self.connection)?;
        let source_files: i64 = self.connection.query_row(
            "SELECT count(*) FROM rollout_files WHERE present = 1",
            [],
            |row| row.get(0),
        )?;
        let raw_events: i64 =
            self.connection
                .query_row("SELECT count(*) FROM usage_events", [], |row| row.get(0))?;
        let indexed_events: i64 = self.connection.query_row(
            &format!("{CANONICAL_CTE} SELECT count(*) FROM canonical"),
            [],
            |row| row.get(0),
        )?;
        let (stored_skipped, filtered_parent_events): (i64, i64) = self.connection.query_row(
            "SELECT coalesce(sum(s.skipped_events), 0),
                    coalesce(sum(s.filtered_parent_events), 0)
             FROM rollout_parser_state s
             JOIN rollout_files f ON f.id = s.file_id
             JOIN session_snapshots snapshot ON snapshot.rollout_key_hash = f.rollout_key_hash
             WHERE snapshot.file_id IS NULL OR snapshot.file_id = s.file_id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let incomplete_files: i64 = self.connection.query_row(
            "SELECT count(*) FROM rollout_files
             WHERE present = 1 AND incomplete_tail = 1",
            [],
            |row| row.get(0),
        )?;
        let duplicate_events = raw_events - indexed_events;
        let warnings = query_warnings(&self.connection, incomplete_files, filtered_parent_events)?;

        Ok(LocalUsageSummary {
            generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            source_files,
            indexed_events,
            skipped_events: stored_skipped + incomplete_files + duplicate_events,
            filtered_parent_events,
            total,
            today,
            by_model,
            by_project,
            warnings,
        })
    }

    /// Returns canonical, privacy-safe local events for all indexed days.
    /// Callers that show a task must still use their own neutral task label.
    pub fn local_events(&self) -> IndexResult<Vec<LocalUsageEvent>> {
        query_local_events(&self.connection, None)
    }

    pub fn local_events_for_date(&self, date: &str) -> IndexResult<Vec<LocalUsageEvent>> {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|_| IndexError::InvalidState("invalid local usage date"))?;
        let start_naive = date
            .and_hms_opt(0, 0, 0)
            .ok_or(IndexError::InvalidState("invalid local date"))?;
        let end_naive = date
            .succ_opt()
            .and_then(|next| next.and_hms_opt(0, 0, 0))
            .ok_or(IndexError::InvalidState("invalid next local date"))?;
        let start = Local
            .from_local_datetime(&start_naive)
            .earliest()
            .ok_or(IndexError::InvalidState("local midnight does not exist"))?;
        let end =
            Local
                .from_local_datetime(&end_naive)
                .latest()
                .ok_or(IndexError::InvalidState(
                    "next local midnight does not exist",
                ))?;
        query_local_events(
            &self.connection,
            Some((start.timestamp_millis(), end.timestamp_millis())),
        )
    }

    #[cfg(test)]
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    fn ensure_file(
        &mut self,
        path_key: &[u8],
        rollout_key_hash: &[u8],
        creation_ms: Option<i64>,
        size_bytes: i64,
        mtime_ms: Option<i64>,
    ) -> IndexResult<FileRow> {
        if let Some(row) = load_file_row(&self.connection, path_key)? {
            return Ok(row);
        }

        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO rollout_files(
               path_key, rollout_key_hash, creation_ms, size_bytes, mtime_ms,
               parser_version, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                path_key,
                rollout_key_hash,
                creation_ms,
                size_bytes,
                mtime_ms,
                PARSER_SEMANTICS_VERSION,
                Utc::now().timestamp_millis(),
            ],
        )?;
        let id = transaction.last_insert_rowid();
        save_state(&transaction, id, &initial_state())?;
        transaction.commit()?;
        load_file_row(&self.connection, path_key)?
            .ok_or(IndexError::InvalidState("new rollout file row disappeared"))
    }

    fn requires_reset(
        &self,
        path: &Path,
        file: &FileRow,
        creation_ms: Option<i64>,
        current_size: i64,
    ) -> IndexResult<bool> {
        if file.parser_version != PARSER_SEMANTICS_VERSION {
            return Ok(true);
        }
        if current_size < file.committed_offset {
            return Ok(true);
        }
        if file.creation_ms.is_some() && creation_ms.is_some() && file.creation_ms != creation_ms {
            return Ok(true);
        }
        if file.prefix_len > 0 && hash_segment(path, 0, file.prefix_len)? != file.prefix_hash {
            return Ok(true);
        }
        if file.guard_len > 0
            && hash_segment(path, file.guard_offset, file.guard_len)? != file.guard_hash
        {
            return Ok(true);
        }
        Ok(false)
    }

    fn load_state(&self, file_id: i64) -> IndexResult<ParserState> {
        load_state(&self.connection, file_id)
    }
}

fn load_file_row(connection: &Connection, path_key: &[u8]) -> IndexResult<Option<FileRow>> {
    connection
        .query_row(
            "SELECT id, creation_ms, committed_offset, committed_lines,
                    prefix_len, prefix_hash, guard_offset, guard_len, guard_hash,
                    source_stamp, parser_version, incomplete_tail
             FROM rollout_files WHERE path_key = ?1",
            [path_key],
            |row| {
                Ok(FileRow {
                    id: row.get(0)?,
                    creation_ms: row.get(1)?,
                    committed_offset: row.get(2)?,
                    committed_lines: row.get(3)?,
                    prefix_len: row.get(4)?,
                    prefix_hash: row.get(5)?,
                    guard_offset: row.get(6)?,
                    guard_len: row.get(7)?,
                    guard_hash: row.get(8)?,
                    source_stamp: row.get(9)?,
                    parser_version: row.get(10)?,
                    incomplete_tail: row.get::<_, i64>(11)? != 0,
                })
            },
        )
        .optional()
        .map_err(IndexError::from)
}

fn load_state(connection: &Connection, file_id: i64) -> IndexResult<ParserState> {
    let raw = connection.query_row(
        "SELECT
           s.first_record_seen, s.next_history_ordinal, s.is_subagent,
           s.ownership_mode, s.boundary_ordinal, s.owned,
           cm.key, cm.label, cp.key, cp.label,
           lm.key, lm.label, lp.key, lp.label,
           s.previous_input_tokens, s.previous_cached_input_tokens,
           s.previous_output_tokens, s.previous_reasoning_output_tokens,
           s.previous_total_tokens, s.last_fallback_pending,
           s.skipped_events, s.filtered_parent_events
         FROM rollout_parser_state s
         JOIN models cm ON cm.id = s.current_model_id
         JOIN projects cp ON cp.id = s.current_project_id
         LEFT JOIN models lm ON lm.id = s.legacy_model_id
         LEFT JOIN projects lp ON lp.id = s.legacy_project_id
         WHERE s.file_id = ?1",
        [file_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Vec<u8>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<Vec<u8>>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<i64>>(14)?,
                row.get::<_, Option<i64>>(15)?,
                row.get::<_, Option<i64>>(16)?,
                row.get::<_, Option<i64>>(17)?,
                row.get::<_, Option<i64>>(18)?,
                row.get::<_, i64>(19)?,
                row.get::<_, i64>(20)?,
                row.get::<_, i64>(21)?,
            ))
        },
    )?;

    let ownership_mode =
        OwnershipMode::parse(&raw.3).ok_or(IndexError::InvalidState("unknown ownership mode"))?;
    let previous_fields = [raw.14, raw.15, raw.16, raw.17, raw.18];
    let previous_total = if previous_fields.iter().all(Option::is_none) {
        None
    } else if previous_fields.iter().all(Option::is_some) {
        Some(TokenUsage {
            input_tokens: raw.14.unwrap_or_default(),
            cached_input_tokens: raw.15.unwrap_or_default(),
            output_tokens: raw.16.unwrap_or_default(),
            reasoning_output_tokens: raw.17.unwrap_or_default(),
            total_tokens: raw.18.unwrap_or_default(),
        })
    } else {
        return Err(IndexError::InvalidState(
            "partial cumulative token baseline",
        ));
    };
    let legacy_candidate = match (raw.10, raw.11, raw.12, raw.13) {
        (Some(model_key), Some(model_label), Some(project_key), Some(project_label)) => {
            Some(Attribution {
                model_key,
                model_label,
                project_key,
                project_label,
            })
        }
        (None, None, None, None) => None,
        _ => return Err(IndexError::InvalidState("partial legacy attribution")),
    };

    Ok(ParserState {
        first_record_seen: raw.0 != 0,
        next_history_ordinal: raw.1,
        is_subagent: raw.2 != 0,
        ownership_mode,
        boundary_ordinal: raw.4,
        owned: raw.5 != 0,
        current: Attribution {
            model_key: raw.6,
            model_label: raw.7,
            project_key: raw.8,
            project_label: raw.9,
        },
        legacy_candidate,
        previous_total,
        last_fallback_pending: raw.19 != 0,
        skipped_events: raw.20,
        filtered_parent_events: raw.21,
    })
}

fn save_state(transaction: &Transaction<'_>, file_id: i64, state: &ParserState) -> IndexResult<()> {
    let current_model_id = ensure_model(transaction, &state.current)?;
    let current_project_id = ensure_project(transaction, &state.current)?;
    let (legacy_model_id, legacy_project_id) = if let Some(candidate) = &state.legacy_candidate {
        (
            Some(ensure_model(transaction, candidate)?),
            Some(ensure_project(transaction, candidate)?),
        )
    } else {
        (None, None)
    };
    let previous = state.previous_total;
    transaction.execute(
        "INSERT INTO rollout_parser_state(
           file_id, first_record_seen, next_history_ordinal, is_subagent,
           ownership_mode, boundary_ordinal, owned,
           current_model_id, current_project_id, legacy_model_id, legacy_project_id,
           previous_input_tokens, previous_cached_input_tokens,
           previous_output_tokens, previous_reasoning_output_tokens,
           previous_total_tokens, last_fallback_pending, skipped_events,
           filtered_parent_events
         ) VALUES (
           ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
           ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19
         )
         ON CONFLICT(file_id) DO UPDATE SET
           first_record_seen = excluded.first_record_seen,
           next_history_ordinal = excluded.next_history_ordinal,
           is_subagent = excluded.is_subagent,
           ownership_mode = excluded.ownership_mode,
           boundary_ordinal = excluded.boundary_ordinal,
           owned = excluded.owned,
           current_model_id = excluded.current_model_id,
           current_project_id = excluded.current_project_id,
           legacy_model_id = excluded.legacy_model_id,
           legacy_project_id = excluded.legacy_project_id,
           previous_input_tokens = excluded.previous_input_tokens,
           previous_cached_input_tokens = excluded.previous_cached_input_tokens,
           previous_output_tokens = excluded.previous_output_tokens,
           previous_reasoning_output_tokens = excluded.previous_reasoning_output_tokens,
           previous_total_tokens = excluded.previous_total_tokens,
           last_fallback_pending = excluded.last_fallback_pending,
           skipped_events = excluded.skipped_events,
           filtered_parent_events = excluded.filtered_parent_events",
        params![
            file_id,
            i64::from(state.first_record_seen),
            state.next_history_ordinal,
            i64::from(state.is_subagent),
            state.ownership_mode.as_str(),
            state.boundary_ordinal,
            i64::from(state.owned),
            current_model_id,
            current_project_id,
            legacy_model_id,
            legacy_project_id,
            previous.map(|usage| usage.input_tokens),
            previous.map(|usage| usage.cached_input_tokens),
            previous.map(|usage| usage.output_tokens),
            previous.map(|usage| usage.reasoning_output_tokens),
            previous.map(|usage| usage.total_tokens),
            i64::from(state.last_fallback_pending),
            state.skipped_events,
            state.filtered_parent_events,
        ],
    )?;
    Ok(())
}

fn ensure_model(connection: &Connection, attribution: &Attribution) -> IndexResult<i64> {
    connection.execute(
        "INSERT INTO models(key, label) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET label = excluded.label",
        params![attribution.model_key, attribution.model_label],
    )?;
    Ok(connection.query_row(
        "SELECT id FROM models WHERE key = ?1",
        [&attribution.model_key],
        |row| row.get(0),
    )?)
}

fn ensure_project(connection: &Connection, attribution: &Attribution) -> IndexResult<i64> {
    connection.execute(
        "INSERT INTO projects(key, label) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET label = excluded.label",
        params![attribution.project_key, attribution.project_label],
    )?;
    Ok(connection.query_row(
        "SELECT id FROM projects WHERE key = ?1",
        [&attribution.project_key],
        |row| row.get(0),
    )?)
}

fn insert_event(
    transaction: &Transaction<'_>,
    file_id: i64,
    event: &PendingUsageEvent,
) -> IndexResult<i64> {
    let model_id = ensure_model(transaction, &event.attribution)?;
    let project_id = ensure_project(transaction, &event.attribution)?;
    let changed = transaction.execute(
        "INSERT OR IGNORE INTO usage_events(
           file_id, logical_event_key, source_ordinal, source_line, end_offset,
           occurred_at_ms, source_kind, model_id, project_id,
           input_tokens, cached_input_tokens, output_tokens,
           reasoning_output_tokens, total_tokens, cache_metrics_available, payload_hash
         ) VALUES (
           ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
           ?10, ?11, ?12, ?13, ?14, ?15, ?16
         )",
        params![
            file_id,
            event.logical_key,
            event.source_ordinal,
            event.source_line,
            event.end_offset,
            event.occurred_at_ms,
            event.source.as_str(),
            model_id,
            project_id,
            event.usage.input_tokens,
            event.usage.cached_input_tokens,
            event.usage.output_tokens,
            event.usage.reasoning_output_tokens,
            event.usage.total_tokens,
            i64::from(event.cache_metrics_available),
            event.payload_hash,
        ],
    )?;
    Ok(i64::try_from(changed).unwrap_or(i64::MAX))
}

fn read_increment(
    path: &Path,
    start_offset: i64,
    start_lines: i64,
    state: &mut ParserState,
    rollout_key_hash: &[u8],
) -> IndexResult<ReadResult> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start_offset as u64))?;
    read_stream(
        BufReader::with_capacity(64 * 1024, file),
        start_offset,
        start_lines,
        state,
        rollout_key_hash,
    )
}

fn read_stream(
    mut reader: impl BufRead,
    start_offset: i64,
    start_lines: i64,
    state: &mut ParserState,
    rollout_key_hash: &[u8],
) -> IndexResult<ReadResult> {
    let mut line = Vec::new();
    let mut oversized = false;
    let mut scanned_offset = start_offset;
    let mut committed_offset = start_offset;
    let mut complete_lines_added = 0_i64;
    let mut events = Vec::new();
    let mut warnings = BTreeMap::new();

    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(available.len());
        if !oversized {
            if line.len().saturating_add(take) > MAX_LINE_BYTES {
                line.clear();
                oversized = true;
            } else {
                line.extend_from_slice(&available[..take]);
            }
        }
        let consumed = take + usize::from(newline.is_some());
        reader.consume(consumed);
        scanned_offset += i64::try_from(consumed)
            .map_err(|_| IndexError::InvalidState("line offset overflow"))?;

        if newline.is_none() {
            continue;
        }

        if !oversized && line.last() == Some(&b'\r') {
            line.pop();
        }
        complete_lines_added += 1;
        let source_line = start_lines + complete_lines_added;
        let effects = process_complete_line(
            state,
            &line,
            oversized,
            source_line,
            scanned_offset,
            rollout_key_hash,
        );
        if let Some(event) = effects.event {
            events.push(event);
        }
        for code in effects.warning_codes {
            let entry = warnings.entry(code).or_insert((0_i64, source_line));
            entry.0 += 1;
            entry.1 = source_line;
        }
        committed_offset = scanned_offset;
        line.clear();
        oversized = false;
    }

    Ok(ReadResult {
        state: state.clone(),
        events,
        warnings,
        committed_offset,
        complete_lines_added,
        incomplete_tail: scanned_offset > committed_offset,
    })
}

fn hash_segment(path: &Path, offset: i64, length: i64) -> IndexResult<Option<Vec<u8>>> {
    if length == 0 {
        return Ok(None);
    }
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset as u64))?;
    let mut bytes = vec![0_u8; length as usize];
    file.read_exact(&mut bytes)?;
    Ok(Some(hash_bytes(&bytes).to_vec()))
}

fn path_key(path: &Path) -> IndexResult<Vec<u8>> {
    let absolute = match fs::canonicalize(path) {
        Ok(path) => path,
        Err(_) if path.is_absolute() => path.to_path_buf(),
        Err(_) => std::env::current_dir()?.join(path),
    };
    let mut normalized = absolute.to_string_lossy().replace('/', r"\");
    if let Some(stripped) = normalized.strip_prefix(r"\\?\") {
        normalized = stripped.to_owned();
    }
    if cfg!(windows) {
        normalized = normalized.to_lowercase();
    }
    Ok(hash_bytes(normalized.as_bytes()).to_vec())
}

fn rollout_key_hash(path: &Path) -> IndexResult<Vec<u8>> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(IndexError::InvalidPath)?;
    let lower_name = name.to_lowercase();
    let lower = lower_name
        .strip_suffix(".jsonl.zst")
        .or_else(|| lower_name.strip_suffix(".jsonl"))
        .unwrap_or(&lower_name);
    let key = lower
        .get(lower.len().saturating_sub(36)..)
        .filter(|candidate| is_uuid(candidate))
        .unwrap_or(lower);
    Ok(hash_bytes(key.as_bytes()).to_vec())
}

fn is_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    value.bytes().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

fn system_time_ms(value: SystemTime) -> Option<i64> {
    let duration = value.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_millis()).ok()
}

fn is_compressed(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".jsonl.zst"))
}

fn source_stamp(metadata: &fs::Metadata) -> String {
    let nanos = |time: std::io::Result<SystemTime>| {
        time.ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
    };
    format!(
        "{}:{:?}:{:?}",
        metadata.len(),
        nanos(metadata.modified()),
        nanos(metadata.created())
    )
}

fn backup_before_migration(connection: &Connection, database_path: &Path) -> IndexResult<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let application_id: i64 =
        connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    if version <= 0
        || version >= super::schema::SCHEMA_VERSION
        || application_id != super::schema::APPLICATION_ID
    {
        return Ok(());
    }
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));
    let directory = parent.join("index-migration-backups");
    fs::create_dir_all(&directory)?;
    let filename = database_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(IndexError::InvalidPath)?;
    let destination = directory.join(format!(
        "{}-schema{version}-{filename}",
        Utc::now().format("%Y%m%dT%H%M%S%.9fZ")
    ));
    // SQLite's backup API includes committed WAL pages and yields one
    // consistent database. A failed backup aborts opening/migration.
    connection.backup(rusqlite::DatabaseName::Main, destination, None)?;
    Ok(())
}

fn discover_rollouts(directory: &Path, files: &mut Vec<PathBuf>) -> IndexResult<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            discover_rollouts(&entry.path(), files)?;
        } else if file_type.is_file() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("rollout-")
                && (name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
            {
                files.push(entry.path());
            }
        }
    }
    Ok(())
}

fn query_usage(connection: &Connection, range: Option<(i64, i64)>) -> IndexResult<TokenUsage> {
    let query = if range.is_some() {
        format!("{CANONICAL_CTE}\n{USAGE_SELECT} WHERE e.occurred_at_ms >= ?1 AND e.occurred_at_ms < ?2")
    } else {
        format!("{CANONICAL_CTE}\n{USAGE_SELECT}")
    };
    let read = |row: &rusqlite::Row<'_>| {
        Ok(TokenUsage {
            input_tokens: row.get::<_, Option<i64>>(0)?.unwrap_or(0),
            cached_input_tokens: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
            output_tokens: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            reasoning_output_tokens: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            total_tokens: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
        })
    };
    Ok(match range {
        Some((start, end)) => connection.query_row(&query, params![start, end], read)?,
        None => connection.query_row(&query, [], read)?,
    })
}

fn query_local_events(
    connection: &Connection,
    range: Option<(i64, i64)>,
) -> IndexResult<Vec<LocalUsageEvent>> {
    let mut query = format!(
        r#"{CANONICAL_CTE}
SELECT e.logical_event_key, e.occurred_at_ms, m.label, p.label,
       e.input_tokens, e.cached_input_tokens, e.output_tokens,
       e.reasoning_output_tokens, e.total_tokens, e.cache_metrics_available
FROM usage_events e
JOIN canonical c ON c.id = e.id
JOIN models m ON m.id = e.model_id
JOIN projects p ON p.id = e.project_id"#
    );
    if range.is_some() {
        query.push_str(" WHERE e.occurred_at_ms >= ?1 AND e.occurred_at_ms < ?2");
    }
    query.push_str(" ORDER BY e.occurred_at_ms ASC, e.id ASC");
    let mut statement = connection.prepare(&query)?;
    let read = |row: &rusqlite::Row<'_>| {
        let key: Vec<u8> = row.get(0)?;
        Ok(LocalUsageEvent {
            task_key: hex(&key),
            occurred_at_ms: row.get(1)?,
            model_label: row.get(2)?,
            project_label: row.get(3)?,
            usage: TokenUsage {
                input_tokens: row.get(4)?,
                cached_input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                reasoning_output_tokens: row.get(7)?,
                total_tokens: row.get(8)?,
            },
            cache_metrics_available: row.get::<_, i64>(9)? != 0,
        })
    };
    let rows = match range {
        Some((start, end)) => statement.query_map(params![start, end], read)?,
        None => statement.query_map([], read)?,
    };
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

const CANONICAL_CTE: &str = r#"
WITH canonical AS (
  SELECT min(e.id) AS id
  FROM usage_events e
  JOIN rollout_files f ON f.id = e.file_id
  JOIN session_snapshots snapshot ON snapshot.rollout_key_hash = f.rollout_key_hash
  WHERE snapshot.file_id IS NULL OR snapshot.file_id = e.file_id
  GROUP BY e.logical_event_key
)
"#;

const USAGE_SELECT: &str = r#"
SELECT sum(e.input_tokens), sum(e.cached_input_tokens), sum(e.output_tokens),
       sum(e.reasoning_output_tokens), sum(e.total_tokens)
FROM usage_events e JOIN canonical c ON c.id = e.id
"#;

fn query_model_dimensions(connection: &Connection) -> IndexResult<Vec<UsageDimension>> {
    let query = format!(
        r#"{CANONICAL_CTE}
SELECT m.key, m.label, sum(e.input_tokens), sum(e.cached_input_tokens),
       sum(e.output_tokens), sum(e.reasoning_output_tokens), sum(e.total_tokens)
FROM usage_events e
JOIN canonical c ON c.id = e.id
JOIN models m ON m.id = e.model_id
GROUP BY m.id, m.key, m.label
ORDER BY sum(e.total_tokens) DESC, m.label ASC"#
    );
    let mut statement = connection.prepare(&query)?;
    let rows = statement.query_map([], |row| {
        Ok(UsageDimension {
            key: row.get(0)?,
            label: row.get(1)?,
            usage: TokenUsage {
                input_tokens: row.get(2)?,
                cached_input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                reasoning_output_tokens: row.get(5)?,
                total_tokens: row.get(6)?,
            },
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn query_project_dimensions(connection: &Connection) -> IndexResult<Vec<UsageDimension>> {
    let query = format!(
        r#"{CANONICAL_CTE}
SELECT p.key, p.label, sum(e.input_tokens), sum(e.cached_input_tokens),
       sum(e.output_tokens), sum(e.reasoning_output_tokens), sum(e.total_tokens)
FROM usage_events e
JOIN canonical c ON c.id = e.id
JOIN projects p ON p.id = e.project_id
GROUP BY p.id, p.key, p.label
ORDER BY sum(e.total_tokens) DESC, p.label ASC"#
    );
    let mut statement = connection.prepare(&query)?;
    let rows = statement.query_map([], |row| {
        let key: Vec<u8> = row.get(0)?;
        Ok(UsageDimension {
            key: hex(&key),
            label: row.get(1)?,
            usage: TokenUsage {
                input_tokens: row.get(2)?,
                cached_input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                reasoning_output_tokens: row.get(5)?,
                total_tokens: row.get(6)?,
            },
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn query_warnings(
    connection: &Connection,
    incomplete_files: i64,
    filtered_parent_events: i64,
) -> IndexResult<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT w.code, sum(w.count)
         FROM file_warning_counts w
         JOIN rollout_files f ON f.id = w.file_id
         JOIN session_snapshots snapshot ON snapshot.rollout_key_hash = f.rollout_key_hash
         WHERE snapshot.file_id IS NULL OR snapshot.file_id = w.file_id
         GROUP BY w.code ORDER BY w.code",
    )?;
    let stored = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let unresolved_files: i64 = connection.query_row(
        "SELECT count(*) FROM rollout_parser_state s
         JOIN rollout_files f ON f.id = s.file_id
         JOIN session_snapshots snapshot ON snapshot.rollout_key_hash = f.rollout_key_hash
         WHERE (snapshot.file_id IS NULL OR snapshot.file_id = s.file_id)
           AND s.is_subagent = 1 AND s.ownership_mode = 'unresolved'",
        [],
        |row| row.get(0),
    )?;
    let filtered_files: i64 = connection.query_row(
        "SELECT count(*) FROM rollout_parser_state s
         JOIN rollout_files f ON f.id = s.file_id
         JOIN session_snapshots snapshot ON snapshot.rollout_key_hash = f.rollout_key_hash
         WHERE (snapshot.file_id IS NULL OR snapshot.file_id = s.file_id)
           AND s.filtered_parent_events > 0",
        [],
        |row| row.get(0),
    )?;

    let mut messages = Vec::new();
    let missing_sources: i64 = connection.query_row(
        "SELECT count(*) FROM session_snapshots snapshot
         WHERE NOT EXISTS (SELECT 1 FROM rollout_files f
           WHERE f.rollout_key_hash = snapshot.rollout_key_hash AND f.present = 1)",
        [],
        |row| row.get(0),
    )?;
    if missing_sources > 0 {
        messages.push(format!(
            "有 {missing_sources} 个会话的源日志当前不可用，已保留此前成功记录的历史用量。"
        ));
    }
    let failed_compressed: Option<String> = connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = 'compressed-read-failures'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(count) = failed_compressed
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|count| *count > 0)
    {
        messages.push(format!(
            "有 {count} 个压缩日志未能完整读取，已保留其此前成功记录的用量。"
        ));
    }
    let failed_plain: Option<String> = connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = 'plain-read-failures'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(count) = failed_plain
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|count| *count > 0)
    {
        messages.push(format!(
            "有 {count} 个日志暂时无法安全重读，已保留其此前成功记录的用量。"
        ));
    }
    if incomplete_files > 0 {
        messages.push(format!(
            "有 {incomplete_files} 个活动日志末行尚未写完，已延后到下次扫描。"
        ));
    }
    for (code, count) in stored {
        let message = match code.as_str() {
            "invalid-json" => format!("有 {count} 条完整日志记录无法解析，已安全跳过。"),
            "invalid-session-meta" => {
                format!("有 {count} 个日志缺少有效的会话元数据，相关用量已保守处理。")
            }
            "invalid-token-count" => format!("有 {count} 条 Token 记录不完整，已安全跳过。"),
            "last-usage-fallback" => {
                format!("有 {count} 个日志段仅能使用单次 Token 快照，结果按保守方式估算。")
            }
            "last-usage-skipped" => format!("有 {count} 条重复的单次 Token 快照未计入总量。"),
            "cumulative-counter-reset" => {
                format!("检测到 {count} 次累计计数器重置，已从新分段继续统计。")
            }
            _ => format!("索引记录了 {count} 次 {code} 告警。"),
        };
        messages.push(message);
    }
    if filtered_parent_events > 0 {
        messages.push(format!(
            "已在 {filtered_files} 个子代理日志中识别并排除复制的父历史。"
        ));
    }
    if unresolved_files > 0 {
        messages.push(format!(
            "有 {unresolved_files} 个旧版子代理日志缺少可靠边界，不确定用量未计入总量。"
        ));
    }
    Ok(messages)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}
