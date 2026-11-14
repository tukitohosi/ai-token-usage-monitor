use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::DateTime;
use rusqlite::{params, OptionalExtension};

use super::model::{Attribution, ScanSummary, TokenUsage};
use super::parser::hash_bytes;
use super::store::{ensure_model, ensure_project};
use super::{IncrementalIndex, IndexError, IndexResult};

pub(super) const DESKTOP_SCHEMA: &str = "
    CREATE TABLE desktop_usage_events (
        id INTEGER PRIMARY KEY,
        logical_event_key BLOB NOT NULL UNIQUE,
        occurred_at_ms INTEGER NOT NULL,
        model_id INTEGER NOT NULL REFERENCES models(id),
        project_id INTEGER NOT NULL REFERENCES projects(id),
        input_tokens INTEGER NOT NULL,
        cached_input_tokens INTEGER NOT NULL,
        output_tokens INTEGER NOT NULL,
        reasoning_output_tokens INTEGER NOT NULL,
        total_tokens INTEGER NOT NULL,
        cache_metrics_available INTEGER NOT NULL
    );
    CREATE INDEX desktop_usage_date ON desktop_usage_events(occurred_at_ms);
    CREATE TABLE desktop_log_files (
        path_hash BLOB PRIMARY KEY,
        stamp TEXT NOT NULL
    ) WITHOUT ROWID;";

struct DesktopEvent {
    key: [u8; 32],
    time: i64,
    attr: Attribution,
    usage: TokenUsage,
}

fn parse_event(line: &[u8]) -> Option<DesktopEvent> {
    let mut parts = std::str::from_utf8(line).ok()?.split_ascii_whitespace();
    let timestamp = parts.next()?;
    if parts.next()? != "info"
        || parts.next()? != "[ephemeral-generation]"
        || parts.next()? != "ephemeral_generation_token_usage"
    {
        return None;
    }
    // These desktop-owned ephemeral calls have no persisted thread/rollout.
    // Match the emitter and event name, never token-shaped text in tool output.
    let fields: BTreeMap<_, _> = parts.filter_map(|part| part.split_once('=')).collect();
    if fields.get("event")? != &"ephemeral_generation_token_usage"
        || fields.get("status")? != &"success"
    {
        return None;
    }
    let time = DateTime::parse_from_rfc3339(timestamp)
        .ok()?
        .timestamp_millis();
    let feature = *fields.get("feature")?;
    let model = *fields.get("model")?;
    if model.is_empty()
        || model.len() > 160
        || model == "null"
        || !model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._:/".contains(&b))
    {
        return None;
    }
    let count = |key: &str| fields.get(key)?.parse::<i64>().ok().filter(|n| *n >= 0);
    let usage = TokenUsage {
        input_tokens: count("inputTokens")?,
        cached_input_tokens: count("cachedInputTokens")?,
        output_tokens: count("outputTokens")?,
        reasoning_output_tokens: count("reasoningOutputTokens")?,
        total_tokens: count("totalTokens")?,
    };
    if usage.total_tokens == 0
        || usage.input_tokens.checked_add(usage.output_tokens)? != usage.total_tokens
        || usage.cached_input_tokens > usage.input_tokens
        || usage.reasoning_output_tokens > usage.output_tokens
    {
        return None;
    }
    let label = match feature {
        "ambient_suggestions" | "ambient_suggestion_safety" => "Codex 后台建议",
        "thread_title" => "Codex 会话标题",
        _ => "Codex 后台任务",
    };
    let signature = format!(
        "codex-desktop-ephemeral\0{timestamp}\0{feature}\0{model}\0{}\0{}\0{}\0{}",
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.output_tokens,
        usage.reasoning_output_tokens
    );
    Some(DesktopEvent {
        key: hash_bytes(signature.as_bytes()),
        time,
        attr: Attribution {
            model_key: model.to_ascii_lowercase(),
            model_label: model.to_owned(),
            project_key: hash_bytes(format!("codex-desktop\0{label}").as_bytes()).to_vec(),
            project_label: label.to_owned(),
        },
        usage,
    })
}

fn find_logs(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if ty.is_dir() && name.bytes().all(|b| b.is_ascii_digit()) {
            find_logs(&entry.path(), files)?;
        } else if ty.is_file() && name.starts_with("codex-desktop-") && name.ends_with(".log") {
            files.push(entry.path());
        }
    }
    Ok(())
}

impl IncrementalIndex {
    pub(crate) fn scan_codex_desktop_data(
        &mut self,
        local_app_data: &Path,
    ) -> IndexResult<ScanSummary> {
        let mut roots = vec![local_app_data.join("Codex").join("Logs")];
        let mut report = ScanSummary::default();
        match fs::read_dir(local_app_data.join("Packages")) {
            Ok(packages) => {
                for entry in packages {
                    match entry {
                        Ok(entry)
                            if entry
                                .file_name()
                                .to_string_lossy()
                                .to_ascii_lowercase()
                                .starts_with("openai.codex_") =>
                        {
                            roots.push(
                                entry
                                    .path()
                                    .join("LocalCache")
                                    .join("Local")
                                    .join("Codex")
                                    .join("Logs"),
                            );
                        }
                        Ok(_) => (),
                        Err(_) => report.incomplete_files += 1,
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(_) => report.incomplete_files += 1,
        }
        let mut seen = HashSet::new();
        for root in roots {
            // MSIX can expose the same physical directory through both views.
            match same_file::Handle::from_path(&root) {
                Ok(handle) => {
                    if !seen.insert(handle) {
                        continue;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                // Some shares cannot identify a directory. Events retain their
                // source-independent key and remain deduplicated on import.
                Err(_) => (),
            }
            match self.scan_codex_desktop_logs(&root) {
                Ok(scan) => {
                    report.discovered_files += scan.discovered_files;
                    report.changed_files += scan.changed_files;
                    report.incomplete_files += scan.incomplete_files;
                    report.indexed_events_added += scan.indexed_events_added;
                }
                Err(IndexError::Io(_)) => report.incomplete_files += 1,
                Err(e) => return Err(e),
            }
        }
        Ok(report)
    }

    pub(crate) fn scan_codex_desktop_logs(&mut self, root: &Path) -> IndexResult<ScanSummary> {
        let mut files = Vec::new();
        match find_logs(root, &mut files) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            other => other?,
        }
        let mut summary = ScanSummary {
            discovered_files: files.len(),
            ..ScanSummary::default()
        };
        for path in files {
            match self.scan_desktop_log(&path) {
                Ok(Some((inserted, tail))) => {
                    summary.changed_files += 1;
                    summary.indexed_events_added += inserted;
                    summary.incomplete_files += usize::from(tail);
                }
                Ok(None) => (),
                // A rotated/locked log can be retried on the next scan.
                Err(IndexError::Io(_)) => summary.incomplete_files += 1,
                Err(e) => return Err(e),
            }
        }
        Ok(summary)
    }

    fn scan_desktop_log(&mut self, path: &Path) -> IndexResult<Option<(i64, bool)>> {
        let meta = fs::metadata(path)?;
        let ns = meta
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let stamp = format!("{}:{ns}", meta.len());
        let key = hash_bytes(path.to_string_lossy().as_bytes());
        let known: Option<String> = self
            .connection
            .query_row(
                "SELECT stamp FROM desktop_log_files WHERE path_hash=?1",
                [&key[..]],
                |r| r.get(0),
            )
            .optional()?;
        if known.as_ref() == Some(&stamp) {
            return Ok(None);
        }
        let mut reader = BufReader::new(File::open(path)?.take(meta.len()));
        let mut buffer = Vec::new();
        let mut events = Vec::new();
        let mut long_line = false;
        let mut tail = false;
        loop {
            buffer.clear();
            // A log may also contain long, unrelated diagnostics. Hold at most
            // one short record in memory, and discard oversized lines in chunks.
            let read = reader.by_ref().take(8192).read_until(b'\n', &mut buffer)?;
            if read == 0 {
                break;
            }
            tail = !buffer.ends_with(b"\n");
            if !long_line && !tail {
                if let Some(event) = parse_event(&buffer) {
                    events.push(event);
                }
            }
            long_line = tail;
        }
        let tx = self.connection.transaction()?;
        let mut added = 0;
        for event in events {
            let model = ensure_model(&tx, &event.attr)?;
            let project = ensure_project(&tx, &event.attr)?;
            let u = event.usage;
            added += tx.execute(
                "INSERT INTO desktop_usage_events
                 (logical_event_key, occurred_at_ms, model_id, project_id, input_tokens, cached_input_tokens,
                  output_tokens, reasoning_output_tokens, total_tokens, cache_metrics_available)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1) ON CONFLICT(logical_event_key) DO NOTHING",
                params![&event.key[..], event.time, model, project, u.input_tokens,
                    u.cached_input_tokens, u.output_tokens, u.reasoning_output_tokens, u.total_tokens],
            )? as i64;
        }
        tx.execute(
            "INSERT INTO desktop_log_files(path_hash,stamp) VALUES (?1,?2)
                    ON CONFLICT(path_hash) DO UPDATE SET stamp=excluded.stamp",
            params![&key[..], stamp],
        )?;
        tx.commit()?;
        Ok(Some((added, tail)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const LINE: &str = "2000-01-01T00:00:00.000Z info [ephemeral-generation] ephemeral_generation_token_usage cachedInputTokens=40 event=ephemeral_generation_token_usage feature=ambient_suggestions inputTokens=100 model=example-model outputTokens=20 reasoningEffort=medium reasoningOutputTokens=5 serviceTier=null status=success totalTokens=120";

    #[test]
    fn desktop_counts_require_terminal_valid_counters() {
        let e = parse_event(LINE.as_bytes()).unwrap();
        assert_eq!(e.usage.input_tokens - e.usage.cached_input_tokens, 60);
        assert_eq!(e.usage.output_tokens - e.usage.reasoning_output_tokens, 15);
        assert!(parse_event(
            LINE.replace("totalTokens=120", "totalTokens=42")
                .as_bytes()
        )
        .is_none());
        assert!(parse_event(LINE.replace("status=success", "status=failed").as_bytes()).is_none());
        assert!(parse_event(format!("other message {LINE}").as_bytes()).is_none());
    }

    #[test]
    fn desktop_events_survive_rotation_and_parse_completed_tails_once() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("codex-desktop-a.log");
        fs::write(&path, LINE).unwrap();
        let mut index = IncrementalIndex::open_in_memory().unwrap();
        assert_eq!(
            index
                .scan_codex_desktop_logs(temp.path())
                .unwrap()
                .indexed_events_added,
            0
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\r\n")
            .unwrap();
        assert_eq!(
            index
                .scan_codex_desktop_logs(temp.path())
                .unwrap()
                .indexed_events_added,
            1
        );
        let rotated = temp.path().join("codex-desktop-b.log");
        fs::copy(&path, &rotated).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(
            index
                .scan_codex_desktop_logs(temp.path())
                .unwrap()
                .indexed_events_added,
            0
        );
        let events = index.local_events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usage.total_tokens, 120);
        assert_eq!(events[0].model_label, "example-model");
        assert_eq!(events[0].project_label, "Codex 后台建议");
        assert!(!events[0].day_only);
    }

    #[test]
    fn windows_package_and_standard_log_roots_preserve_distinct_usage() {
        let local = tempfile::tempdir().unwrap();
        let standard = local.path().join("Codex/Logs/2000/01/01");
        let packaged = local
            .path()
            .join("Packages/OpenAI.Codex_example/LocalCache/Local/Codex/Logs/2000/01/01");
        fs::create_dir_all(&packaged).unwrap();
        let log = "codex-desktop-event.log";
        fs::write(packaged.join(log), format!("{LINE}\n")).unwrap();
        let mut index = IncrementalIndex::open_in_memory().unwrap();
        assert_eq!(
            index
                .scan_codex_desktop_data(local.path())
                .unwrap()
                .indexed_events_added,
            1
        );
        fs::create_dir_all(&standard).unwrap();
        let distinct = LINE.replace("00:00:00", "00:01:00");
        fs::write(standard.join(log), format!("{LINE}\n{distinct}\n")).unwrap();
        let second = index.scan_codex_desktop_data(local.path()).unwrap();
        assert_eq!(second.discovered_files, 2);
        assert_eq!(second.indexed_events_added, 1);
        assert_eq!(index.local_events().unwrap().len(), 2);
    }
}
