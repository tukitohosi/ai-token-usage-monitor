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
fn creates_v2_schema_and_wal_database() {
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

    assert_eq!(version, 2);
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
