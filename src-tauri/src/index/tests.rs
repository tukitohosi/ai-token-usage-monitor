use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde_json::{json, Value};
use tempfile::tempdir;

use super::parser::normalize_project;
use super::IncrementalIndex;

fn record(record_type: &str, payload: Value) -> String {
    json!({
        "timestamp": "2026-08-28T00:00:00Z",
        "type": record_type,
        "payload": payload,
    })
    .to_string()
}

fn usage(total: i64) -> Value {
    json!({
        "input_tokens": total,
        "cached_input_tokens": 0,
        "output_tokens": 0,
        "reasoning_output_tokens": 0,
        "total_tokens": total,
    })
}

fn token(total: i64) -> String {
    record(
        "event_msg",
        json!({
            "type": "token_count",
            "info": { "total_token_usage": usage(total) },
        }),
    )
}

fn write_lines(path: &Path, lines: &[String], final_newline: bool) {
    let mut text = lines.join("\n");
    if final_newline {
        text.push('\n');
    }
    fs::write(path, text).expect("write fixture");
}

fn append_text(path: &Path, text: &str) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open fixture for append")
        .write_all(text.as_bytes())
        .expect("append fixture");
}

fn all_time(index: &IncrementalIndex) -> super::LocalUsageSummary {
    index
        .aggregate_for_range(0, i64::MAX)
        .expect("aggregate index")
}

#[test]
fn creates_current_schema_and_wal_database() {
    let temp = tempdir().expect("temp dir");
    let database = temp.path().join("usage.sqlite3");
    let index = IncrementalIndex::open(&database).expect("open index");

    let version: i64 = index
        .connection()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("read user_version");
    let mode: String = index
        .connection()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("read journal mode");
    let tables: i64 = index
        .connection()
        .query_row(
            "SELECT count(*) FROM sqlite_schema
             WHERE type = 'table' AND name IN (
               'rollout_files', 'rollout_parser_state', 'usage_events',
               'models', 'projects'
             )",
            [],
            |row| row.get(0),
        )
        .expect("read schema");

    assert_eq!(version, super::SCHEMA_VERSION);
    assert_eq!(mode.to_lowercase(), "wal");
    assert_eq!(tables, 5);
}

#[test]
fn canonicalizes_equivalent_windows_project_paths_without_persisting_them() {
    let first = normalize_project(r"\\?\C:\Work\Foo\..\Project\");
    let second = normalize_project("c:/work/project");
    assert_eq!(first.0, second.0);
    assert_eq!(first.1, "Project");
    assert_eq!(second.1, "project");
}

#[test]
fn appends_incrementally_and_repeated_scan_is_idempotent() {
    let temp = tempdir().expect("temp dir");
    let rollout = temp.path().join("rollout-incremental.jsonl");
    write_lines(
        &rollout,
        &[
            record("session_meta", json!({ "cwd": "C:\\Work\\Project" })),
            record(
                "turn_context",
                json!({ "cwd": "C:\\Work\\Project", "model": "gpt-test" }),
            ),
            token(10),
        ],
        true,
    );
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();

    let first = index.scan_file(&rollout).expect("first scan");
    let second = index.scan_file(&rollout).expect("idempotent scan");
    append_text(&rollout, &format!("{}\n", token(15)));
    let third = index.scan_file(&rollout).expect("append scan");
    let summary = all_time(&index);

    assert_eq!(first.indexed_events_added, 1);
    assert_eq!(second.complete_lines_added, 0);
    assert_eq!(second.indexed_events_added, 0);
    assert_eq!(third.complete_lines_added, 1);
    assert_eq!(third.indexed_events_added, 1);
    assert_eq!(summary.indexed_events, 2);
    assert_eq!(summary.total.total_tokens, 15);
    assert_eq!(summary.by_model[0].label, "gpt-test");
    assert_eq!(summary.by_project[0].label, "Project");
}

#[test]
fn deduplicates_copied_rollout_files_by_uuid_and_ordinal() {
    let temp = tempdir().expect("temp dir");
    let first = temp
        .path()
        .join("rollout-first-00000000-0000-0000-0000-000000000999.jsonl");
    let second = temp
        .path()
        .join("rollout-copy-00000000-0000-0000-0000-000000000999.jsonl");
    let lines = [record("session_meta", json!({})), token(8)];
    write_lines(&first, &lines, true);
    write_lines(&second, &lines, true);
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();

    index.scan_file(&first).unwrap();
    index.scan_file(&second).unwrap();
    let summary = all_time(&index);

    assert_eq!(summary.source_files, 2);
    assert_eq!(summary.indexed_events, 1);
    assert_eq!(summary.total.total_tokens, 8);
    assert_eq!(summary.skipped_events, 1);
}

#[test]
fn readonly_export_preserves_canonical_deduplication_and_never_migrates() {
    let temp = tempdir().unwrap();
    let database = temp.path().join("usage.sqlite3");
    let first = temp
        .path()
        .join("rollout-first-00000000-0000-0000-0000-000000000999.jsonl");
    let copied = temp
        .path()
        .join("rollout-copy-00000000-0000-0000-0000-000000000999.jsonl");
    let lines = [record("session_meta", json!({})), token(8)];
    write_lines(&first, &lines, true);
    write_lines(&copied, &lines, true);
    {
        let mut index = IncrementalIndex::open(&database).unwrap();
        index.scan_file(&first).unwrap();
        index.scan_file(&copied).unwrap();
    }
    let before = fs::read(&database).unwrap();
    let events = IncrementalIndex::read_existing_events(&database, None).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].usage.total_tokens, 8);
    let timestamp = events[0].occurred_at_ms.unwrap();
    assert_eq!(
        IncrementalIndex::read_existing_events(&database, Some((timestamp, timestamp + 1)))
            .unwrap()
            .len(),
        1
    );
    assert!(IncrementalIndex::read_existing_events(
        &database,
        Some((timestamp + 1, timestamp + 2))
    )
    .unwrap()
    .is_empty());
    assert_eq!(fs::read(&database).unwrap(), before);
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .pragma_update(None, "user_version", 1_i64)
        .unwrap();
    drop(connection);
    let outdated = fs::read(&database).unwrap();
    assert!(IncrementalIndex::read_existing_events(&database, None).is_err());
    assert_eq!(fs::read(&database).unwrap(), outdated);
    let absent = temp.path().join("does-not-exist.sqlite3");
    assert!(IncrementalIndex::read_existing_events(&absent, None).is_err());
    assert!(!absent.exists());
}

#[test]
fn local_events_expose_an_opaque_task_key_without_raw_rollout_identity() {
    let temp = tempdir().expect("temp dir");
    let rollout = temp
        .path()
        .join("rollout-private-00000000-0000-0000-0000-000000000777.jsonl");
    write_lines(
        &rollout,
        &[
            record(
                "session_meta",
                json!({ "cwd": "C:\\Users\\稚青\\private-project" }),
            ),
            record(
                "turn_context",
                json!({ "model": "gpt-5.6", "cwd": "C:\\Users\\稚青\\private-project" }),
            ),
            token(12),
        ],
        true,
    );
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();
    index.scan_file(&rollout).unwrap();
    let events = index.local_events().expect("local events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].task_key.len(), 64);
    let serialized = serde_json::to_string(&events).expect("serialize events");
    assert!(!serialized.contains("00000000-0000-0000-0000-000000000777"));
    assert!(!serialized.contains("C:\\Users\\稚青"));
    assert_eq!(events[0].project_label, "private-project");
}

#[test]
fn cache_hit_availability_requires_the_cached_input_component() {
    let temp = tempdir().expect("temp dir");
    let rollout = temp.path().join("rollout-cache-fields.jsonl");
    let missing_cached = record(
        "event_msg",
        json!({
            "type": "token_count",
            "info": { "total_token_usage": {
                "input_tokens": 10,
                "output_tokens": 1,
                "total_tokens": 11
            }}
        }),
    );
    write_lines(
        &rollout,
        &[record("session_meta", json!({})), missing_cached],
        true,
    );
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();
    index.scan_file(&rollout).unwrap();
    let events = index.local_events().expect("local events");
    assert_eq!(events.len(), 1);
    assert!(!events[0].cache_metrics_available);
}

#[test]
fn incomplete_tail_never_advances_offset_or_parser_state() {
    let temp = tempdir().expect("temp dir");
    let rollout = temp.path().join("rollout-tail.jsonl");
    let session = record("session_meta", json!({}));
    let pending = token(9);
    fs::write(&rollout, format!("{session}\n{pending}")).unwrap();
    let expected_offset = i64::try_from(session.len() + 1).unwrap();
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();

    let first = index.scan_file(&rollout).expect("scan incomplete tail");
    let before_completion = all_time(&index);
    let second = index.scan_file(&rollout).expect("rescan same tail");
    append_text(&rollout, "\n");
    let completed = index.scan_file(&rollout).expect("complete tail");
    let after_completion = all_time(&index);

    assert!(first.incomplete_tail);
    assert_eq!(first.committed_offset, expected_offset);
    assert_eq!(second.committed_offset, expected_offset);
    assert_eq!(second.complete_lines_added, 0);
    assert_eq!(before_completion.total.total_tokens, 0);
    assert!(!completed.incomplete_tail);
    assert_eq!(completed.complete_lines_added, 1);
    assert_eq!(after_completion.total.total_tokens, 9);
}

#[test]
fn restart_restores_cumulative_baseline_and_offset() {
    let temp = tempdir().expect("temp dir");
    let database = temp.path().join("usage.sqlite3");
    let rollout = temp.path().join("rollout-restart.jsonl");
    write_lines(
        &rollout,
        &[record("session_meta", json!({})), token(10)],
        true,
    );

    {
        let mut index = IncrementalIndex::open(&database).unwrap();
        index.scan_file(&rollout).unwrap();
        assert_eq!(all_time(&index).total.total_tokens, 10);
    }
    append_text(&rollout, &format!("{}\n", token(14)));
    {
        let mut reopened = IncrementalIndex::open(&database).unwrap();
        let report = reopened.scan_file(&rollout).unwrap();
        let summary = all_time(&reopened);
        assert_eq!(report.complete_lines_added, 1);
        assert_eq!(report.indexed_events_added, 1);
        assert_eq!(summary.total.total_tokens, 14);
        assert_eq!(summary.indexed_events, 2);
    }
}

#[test]
fn restart_preserves_subagent_parent_baseline_and_next_ordinal() {
    let temp = tempdir().expect("temp dir");
    let database = temp.path().join("usage.sqlite3");
    let rollout = temp
        .path()
        .join("rollout-prefix-00000000-0000-0000-0000-000000000123.jsonl");
    write_lines(
        &rollout,
        &[
            record(
                "session_meta",
                json!({
                    "parent_thread_id": "parent",
                    "subagent_history_start_ordinal": 1,
                }),
            ),
            record(
                "turn_context",
                json!({ "model": "parent", "cwd": "/parent" }),
            ),
            token(100),
        ],
        true,
    );
    {
        let mut index = IncrementalIndex::open(&database).unwrap();
        index.scan_file(&rollout).unwrap();
        let summary = all_time(&index);
        assert_eq!(summary.total.total_tokens, 0);
        assert_eq!(summary.filtered_parent_events, 1);
    }

    append_text(
        &rollout,
        &format!(
            "{}\n{}\n",
            record("turn_context", json!({ "model": "child", "cwd": "/child" })),
            token(106),
        ),
    );
    let mut reopened = IncrementalIndex::open(&database).unwrap();
    reopened.scan_file(&rollout).unwrap();
    let summary = all_time(&reopened);
    assert_eq!(summary.total.total_tokens, 6);
    assert_eq!(summary.by_model[0].label, "child");
    assert_eq!(summary.by_project[0].label, "child");
}

#[test]
fn new_subagent_boundary_filters_parent_and_uses_parent_baseline() {
    let temp = tempdir().expect("temp dir");
    let rollout = temp.path().join("rollout-subagent.jsonl");
    write_lines(
        &rollout,
        &[
            record(
                "session_meta",
                json!({
                    "parent_thread_id": "must-not-persist",
                    "subagent_history_start_ordinal": 3,
                    "cwd": "C:\\Work\\Parent",
                }),
            ),
            record("session_meta", json!({ "cwd": "C:\\Work\\Parent" })),
            record(
                "turn_context",
                json!({ "model": "parent-model", "cwd": "C:\\Work\\Parent" }),
            ),
            token(480),
            token(500),
            record(
                "turn_context",
                json!({ "model": "child-model", "cwd": "C:\\Work\\Child" }),
            ),
            record(
                "response_item",
                json!({ "type": "message", "content": "TOP SECRET PROMPT" }),
            ),
            token(510),
            token(515),
        ],
        true,
    );
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();
    index.scan_file(&rollout).unwrap();
    let summary = all_time(&index);

    assert_eq!(summary.total.total_tokens, 15);
    assert_eq!(summary.indexed_events, 2);
    assert_eq!(summary.filtered_parent_events, 2);
    assert_eq!(summary.by_model[0].label, "child-model");
    assert_eq!(summary.by_project[0].label, "Child");
}

#[test]
fn damaged_complete_history_line_consumes_one_ordinal() {
    let temp = tempdir().expect("temp dir");
    let rollout = temp.path().join("rollout-damaged.jsonl");
    write_lines(
        &rollout,
        &[
            record(
                "session_meta",
                json!({
                    "parent_thread_id": "parent",
                    "subagent_history_start_ordinal": 1,
                }),
            ),
            "{damaged-history-record}".to_owned(),
            token(500),
            record("turn_context", json!({ "model": "owned", "cwd": "/owned" })),
            token(506),
        ],
        true,
    );
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();
    index.scan_file(&rollout).unwrap();
    let summary = all_time(&index);

    assert_eq!(summary.total.total_tokens, 6);
    assert_eq!(summary.indexed_events, 1);
    assert_eq!(summary.by_model[0].label, "owned");
    assert_eq!(summary.skipped_events, 1);
}

#[test]
fn legacy_marker_preserves_parent_cumulative_baseline() {
    let temp = tempdir().expect("temp dir");
    let rollout = temp.path().join("rollout-legacy.jsonl");
    write_lines(
        &rollout,
        &[
            record(
                "session_meta",
                json!({ "parent_thread_id": "parent", "cwd": "C:\\Repo" }),
            ),
            record(
                "turn_context",
                json!({ "model": "legacy-model", "cwd": "C:\\Repo" }),
            ),
            token(900),
            record(
                "inter_agent_communication_metadata",
                json!({ "trigger_turn": true }),
            ),
            token(907),
        ],
        true,
    );
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();
    index.scan_file(&rollout).unwrap();
    let summary = all_time(&index);

    assert_eq!(summary.total.total_tokens, 7);
    assert_eq!(summary.filtered_parent_events, 1);
    assert_eq!(summary.by_model[0].label, "legacy-model");
}

#[test]
fn database_never_contains_raw_conversation_or_full_path() {
    let temp = tempdir().expect("temp dir");
    let database = temp.path().join("usage.sqlite3");
    let rollout = temp.path().join("rollout-privacy.jsonl");
    let secret = "PROMPT-SENTINEL-8db912c0";
    let parent = "PARENT-ID-SENTINEL-f78c";
    let full_path = r"C:\Users\Private Person\Secret Project";
    write_lines(
        &rollout,
        &[
            record(
                "session_meta",
                json!({ "parent_thread_id": parent, "subagent_history_start_ordinal": 0 }),
            ),
            record(
                "response_item",
                json!({ "content": secret, "email": "private@example.invalid" }),
            ),
            record(
                "turn_context",
                json!({ "model": "safe-model", "cwd": full_path }),
            ),
            token(3),
        ],
        true,
    );
    {
        let mut index = IncrementalIndex::open(&database).unwrap();
        index.scan_file(&rollout).unwrap();
        index
            .connection()
            .execute_batch("PRAGMA wal_checkpoint(FULL);")
            .unwrap();
    }

    for path in [
        database.clone(),
        database.with_extension("sqlite3-wal"),
        database.with_extension("sqlite3-shm"),
    ] {
        let bytes = fs::read(path).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains(secret));
        assert!(!text.contains(parent));
        assert!(!text.contains(full_path));
        assert!(!text.contains("private@example.invalid"));
    }
}

fn write_compressed(path: &Path, lines: &[String]) {
    let text = format!("{}\n", lines.join("\n"));
    let bytes = zstd::stream::encode_all(text.as_bytes(), 1).unwrap();
    fs::write(path, bytes).unwrap();
}

#[test]
fn discovers_archived_compressed_logs_and_deduplicates_plain_copies() {
    let temp = tempdir().unwrap();
    let active = temp.path().join("sessions/2026/08");
    let archive = temp.path().join("archived_sessions");
    fs::create_dir_all(&active).unwrap();
    fs::create_dir_all(&archive).unwrap();
    let name = "rollout-2026-08-28-00000000-0000-0000-0000-000000000111.jsonl";
    let plain = active.join(name);
    let compressed = archive.join(format!("{name}.zst"));
    let lines = [record("session_meta", json!({})), token(12)];
    write_lines(&plain, &lines, true);
    write_compressed(&compressed, &lines);
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();
    let first = index.scan_codex_home(temp.path()).unwrap();
    assert_eq!(first.discovered_files, 2);
    assert_eq!(all_time(&index).total.total_tokens, 12);
    assert_eq!(index.local_events().unwrap().len(), 1);
    let repeated = index.scan_codex_home(temp.path()).unwrap();
    assert_eq!(repeated.changed_files, 0);
    assert_eq!(repeated.indexed_events_added, 0);
    fs::remove_file(plain).unwrap();
    index.scan_codex_home(temp.path()).unwrap();
    assert_eq!(all_time(&index).total.total_tokens, 12);
    assert_eq!(all_time(&index).source_files, 1);
    assert_eq!(index.scan_file(compressed).unwrap().committed_offset, 0);
}

#[test]
fn compression_and_archive_move_keep_the_same_logical_event_ids() {
    let temp = tempdir().unwrap();
    fs::create_dir(temp.path().join("sessions")).unwrap();
    fs::create_dir(temp.path().join("archived_sessions")).unwrap();
    let name = "rollout-00000000-0000-0000-0000-000000000112.jsonl";
    let plain = temp.path().join("sessions").join(name);
    let compressed = temp
        .path()
        .join("archived_sessions")
        .join(format!("{name}.zst"));
    let lines = [record("session_meta", json!({})), token(10), token(15)];
    write_lines(&plain, &lines, true);
    let database = temp.path().join("usage.sqlite3");
    let original_keys = {
        let mut index = IncrementalIndex::open(&database).unwrap();
        index.scan_codex_home(temp.path()).unwrap();
        index
            .local_events()
            .unwrap()
            .into_iter()
            .map(|event| event.task_key)
            .collect::<Vec<_>>()
    };
    write_compressed(&compressed, &lines);
    fs::remove_file(plain).unwrap();
    let mut index = IncrementalIndex::open(&database).unwrap();
    index.scan_codex_home(temp.path()).unwrap();
    assert_eq!(all_time(&index).total.total_tokens, 15);
    let keys = index
        .local_events()
        .unwrap()
        .into_iter()
        .map(|event| event.task_key)
        .collect::<Vec<_>>();
    assert_eq!(keys, original_keys);
}

#[test]
fn deleted_sources_and_missing_directories_preserve_history_even_after_rebuild() {
    let temp = tempdir().unwrap();
    let sessions = temp.path().join("sessions");
    fs::create_dir(&sessions).unwrap();
    let path = sessions.join("rollout-retained.jsonl");
    write_lines(&path, &[record("session_meta", json!({})), token(42)], true);
    let database = temp.path().join("usage.sqlite3");
    let mut index = IncrementalIndex::open(&database).unwrap();
    index.scan_codex_home(temp.path()).unwrap();
    fs::remove_file(path).unwrap();
    fs::remove_dir(sessions).unwrap();
    index.scan_codex_home(temp.path()).unwrap();
    index.invalidate_scan_state_for_rebuild().unwrap();
    index.scan_codex_home(temp.path()).unwrap();
    let summary = all_time(&index);
    assert_eq!(summary.total.total_tokens, 42);
    assert_eq!(summary.indexed_events, 1);
    assert_eq!(summary.source_files, 0);
    assert!(summary
        .warnings
        .iter()
        .any(|message| message.contains("保留")));
    drop(index);
    let index = IncrementalIndex::open(&database).unwrap();
    assert_eq!(all_time(&index).total.total_tokens, 42);
    assert_eq!(
        IncrementalIndex::read_existing_events(&database, None)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn compressed_stamp_change_corrects_snapshot_without_replaying_tokens() {
    let temp = tempdir().unwrap();
    let compressed = temp.path().join("rollout-correction.jsonl.zst");
    write_compressed(
        &compressed,
        &[record("session_meta", json!({})), token(10), token(25)],
    );
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();
    index.scan_file(&compressed).unwrap();
    assert_eq!(all_time(&index).total.total_tokens, 25);
    write_compressed(&compressed, &[record("session_meta", json!({})), token(7)]);
    let report = index.scan_file(&compressed).unwrap();
    assert!(report.reset);
    assert_eq!(report.committed_offset, 0);
    assert_eq!(all_time(&index).total.total_tokens, 7);
    assert_eq!(all_time(&index).indexed_events, 1);
    assert_eq!(
        index.scan_file(&compressed).unwrap().complete_lines_added,
        0
    );
    // A valid empty Zstandard frame is not a complete session snapshot.
    fs::write(
        &compressed,
        zstd::stream::encode_all(b"".as_slice(), 1).unwrap(),
    )
    .unwrap();
    assert!(index.scan_file(&compressed).is_err());
    assert_eq!(all_time(&index).total.total_tokens, 7);
}

#[test]
fn short_new_copy_cannot_hide_a_more_complete_session_snapshot() {
    let temp = tempdir().unwrap();
    let first = temp
        .path()
        .join("rollout-first-00000000-0000-0000-0000-000000000113.jsonl");
    let short = temp
        .path()
        .join("rollout-short-00000000-0000-0000-0000-000000000113.jsonl.zst");
    write_lines(
        &first,
        &[record("session_meta", json!({})), token(10), token(30)],
        true,
    );
    write_compressed(&short, &[record("session_meta", json!({})), token(10)]);
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();
    index.scan_file(&first).unwrap();
    index.scan_file(&short).unwrap();
    assert_eq!(all_time(&index).total.total_tokens, 30);
}

#[test]
fn a_successful_changed_copy_corrects_old_snapshot_and_removes_obsolete_events() {
    let temp = tempdir().unwrap();
    let first = temp
        .path()
        .join("rollout-first-00000000-0000-0000-0000-000000000114.jsonl");
    let copy = temp
        .path()
        .join("rollout-copy-00000000-0000-0000-0000-000000000114.jsonl.zst");
    let original = [record("session_meta", json!({})), token(10), token(30)];
    write_lines(&first, &original, true);
    write_compressed(&copy, &original);
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();
    index.scan_file(&copy).unwrap();
    index.scan_file(&first).unwrap();
    // A previously known compressed file changes to a corrected, shorter
    // valid snapshot. Old events in the other copy must not win deduplication.
    write_compressed(&copy, &[record("session_meta", json!({})), token(7)]);
    index.scan_file(&copy).unwrap();
    index.scan_file(&first).unwrap();
    assert_eq!(all_time(&index).total.total_tokens, 7);
    assert_eq!(all_time(&index).indexed_events, 1);
    index.invalidate_scan_state_for_rebuild().unwrap();
    index.scan_file(&first).unwrap();
    index.scan_file(&copy).unwrap();
    assert_eq!(all_time(&index).total.total_tokens, 7);
}

#[test]
fn truncated_compressed_stream_preserves_all_old_events_and_scan_state() {
    let temp = tempdir().unwrap();
    fs::create_dir(temp.path().join("sessions")).unwrap();
    let compressed = temp.path().join("sessions/rollout-corrupt.jsonl.zst");
    write_compressed(&compressed, &[record("session_meta", json!({})), token(17)]);
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();
    index.scan_codex_home(temp.path()).unwrap();
    let before = index.local_events().unwrap();
    let old_stamp: String = index
        .connection()
        .query_row("SELECT source_stamp FROM rollout_files", [], |row| {
            row.get(0)
        })
        .unwrap();
    // The first frame emits complete, changed usage. A truncated second frame
    // must invalidate the entire stream rather than committing the prefix.
    let mut corrupt = zstd::stream::encode_all(
        format!("{}\n{}\n", record("session_meta", json!({})), token(999)).as_bytes(),
        1,
    )
    .unwrap();
    let second_frame =
        zstd::stream::encode_all(b"a second unfinished frame".as_slice(), 1).unwrap();
    corrupt.extend_from_slice(&second_frame[..second_frame.len() - 1]);
    fs::write(&compressed, corrupt).unwrap();
    assert!(index.scan_file(&compressed).is_err());
    assert_eq!(index.local_events().unwrap(), before);
    let stamp: String = index
        .connection()
        .query_row("SELECT source_stamp FROM rollout_files", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(stamp, old_stamp);
    index.scan_codex_home(temp.path()).unwrap();
    assert_eq!(all_time(&index).total.total_tokens, 17);
    assert!(all_time(&index)
        .warnings
        .iter()
        .any(|message| message.contains("压缩日志")));
}

#[test]
fn malformed_new_copies_cannot_replace_known_session_history() {
    let temp = tempdir().unwrap();
    let uuid = "00000000-0000-0000-0000-000000000116";
    let original = temp.path().join(format!("rollout-good-{uuid}.jsonl"));
    write_lines(
        &original,
        &[record("session_meta", json!({})), token(17)],
        true,
    );
    let mut index = IncrementalIndex::open(temp.path().join("usage.sqlite3")).unwrap();
    index.scan_file(&original).unwrap();
    let before = index.local_events().unwrap();
    let invalid_records = [
        "{invalid-json}".to_owned(),
        record("event_msg", json!({"type":"token_count","info":{}})),
    ];
    for compressed in [false, true] {
        for (case, invalid) in invalid_records.iter().enumerate() {
            let suffix = if compressed { ".jsonl.zst" } else { ".jsonl" };
            let copy = temp
                .path()
                .join(format!("rollout-bad-{compressed}-{case}-{uuid}{suffix}"));
            let lines = [
                record("session_meta", json!({})),
                token(999),
                invalid.clone(),
            ];
            if compressed {
                // This frame is complete; only its decoded JSONL is malformed.
                write_compressed(&copy, &lines);
            } else {
                write_lines(&copy, &lines, true);
            }
            assert!(index.scan_file(&copy).is_err());
            assert_eq!(index.local_events().unwrap(), before);
            assert_eq!(all_time(&index).total.total_tokens, 17);
            let persisted_events: i64 = index
                .connection()
                .query_row("SELECT count(*) FROM usage_events", [], |row| row.get(0))
                .unwrap();
            assert_eq!(persisted_events, 1);
        }
    }
    // The existing active file still commits complete increments while its
    // newest line is only partially written.
    append_text(&original, &format!("{}\n{}", token(19), token(25)));
    let report = index.scan_file(&original).unwrap();
    assert!(report.incomplete_tail);
    assert_eq!(all_time(&index).total.total_tokens, 19);
}

#[test]
fn a_new_session_without_history_still_skips_bad_complete_records() {
    let temp = tempdir().unwrap();
    for compressed in [false, true] {
        let suffix = if compressed { ".jsonl.zst" } else { ".jsonl" };
        let path = temp
            .path()
            .join(format!("rollout-new-{compressed}{suffix}"));
        let lines = [
            record("session_meta", json!({})),
            token(17),
            "{bad}".to_owned(),
        ];
        if compressed {
            write_compressed(&path, &lines);
        } else {
            write_lines(&path, &lines, true);
        }
        let mut index = IncrementalIndex::open_in_memory().unwrap();
        index.scan_file(path).unwrap();
        assert_eq!(all_time(&index).total.total_tokens, 17);
        assert_eq!(all_time(&index).skipped_events, 1);
    }
}

#[test]
fn legacy_schema_migration_backs_up_wal_and_keeps_usage_without_source_logs() {
    let temp = tempdir().unwrap();
    let database = temp.path().join("usage.sqlite3");
    let rollout = temp.path().join("rollout-legacy-upgrade.jsonl");
    write_lines(
        &rollout,
        &[record("session_meta", json!({})), token(83)],
        true,
    );
    {
        let mut index = IncrementalIndex::open(&database).unwrap();
        index.scan_file(&rollout).unwrap();
        index
            .connection()
            .execute_batch(
                "DROP TABLE session_snapshots;
             ALTER TABLE rollout_files DROP COLUMN source_stamp;
             ALTER TABLE usage_events DROP COLUMN cache_metrics_available;
             DELETE FROM schema_migrations WHERE version > 1;
             UPDATE app_meta SET value = '1' WHERE key = 'parser_semantics_version';
             PRAGMA user_version = 1;",
            )
            .unwrap();
    }
    fs::remove_file(&rollout).unwrap();
    // Keep the writer open, so migration must copy committed WAL pages using
    // SQLite instead of copying only the main database file.
    let writer = rusqlite::Connection::open(&database).unwrap();
    writer
        .execute(
            "UPDATE usage_events SET total_tokens = 84, input_tokens = 84",
            [],
        )
        .unwrap();
    let mut index = IncrementalIndex::open(&database).unwrap();
    index.scan_codex_home(temp.path()).unwrap();
    assert_eq!(all_time(&index).total.total_tokens, 84);
    assert!(!index.local_events().unwrap()[0].cache_metrics_available);
    let backups = fs::read_dir(temp.path().join("index-migration-backups"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(backups.len(), 1);
    let backup = rusqlite::Connection::open(backups[0].path()).unwrap();
    assert_eq!(
        backup
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        backup
            .query_row("SELECT sum(total_tokens) FROM usage_events", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        84
    );
    drop(writer);
}

#[test]
fn v2_migration_preserves_legacy_event_union_until_a_complete_reread() {
    let temp = tempdir().unwrap();
    let database = temp.path().join("usage.sqlite3");
    let first = temp
        .path()
        .join("rollout-a-00000000-0000-0000-0000-000000000115.jsonl");
    let second = temp
        .path()
        .join("rollout-b-00000000-0000-0000-0000-000000000115.jsonl");
    write_lines(&first, &[record("session_meta", json!({})), token(8)], true);
    write_lines(
        &second,
        &[
            record("session_meta", json!({})),
            record("response_item", json!({})),
            token(8),
        ],
        true,
    );
    {
        let mut index = IncrementalIndex::open(&database).unwrap();
        index.scan_file(&first).unwrap();
        index.scan_file(&second).unwrap();
        index
            .connection()
            .execute_batch(
                "DROP TABLE session_snapshots;
             ALTER TABLE rollout_files DROP COLUMN source_stamp;
             DELETE FROM schema_migrations WHERE version > 2;
             PRAGMA user_version = 2;",
            )
            .unwrap();
    }
    let mut index = IncrementalIndex::open(&database).unwrap();
    // v2 deduplicated each ordinal across copies, so these different positions
    // were two legacy events. Migration must preserve both until a full reread
    // gives us a replacement snapshot rather than silently dropping one.
    assert_eq!(all_time(&index).total.total_tokens, 16);
    assert_eq!(all_time(&index).indexed_events, 2);
    index.scan_file(&first).unwrap();
    assert_eq!(all_time(&index).total.total_tokens, 16);
    index.invalidate_scan_state_for_rebuild().unwrap();
    index.scan_file(&second).unwrap();
    assert_eq!(all_time(&index).total.total_tokens, 8);
    assert_eq!(all_time(&index).indexed_events, 1);
}
