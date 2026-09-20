//! Privacy-preserving, local-only readers for non-Codex AI usage sources.
//!
//! This module never persists source text, paths, session ids, database rows, or
//! credentials. The v2 database only contains derived counters and irreversible
//! hashes needed for incremental refreshes.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveDate, TimeZone, Timelike, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::{
    index::{LocalUsageEvent, LocalUsageSummary},
    pricing::{self, CostSummary, PriceUsage},
};

pub(crate) const MULTI_SOURCE_SCHEMA_VERSION: i64 = 3;

const SOURCE_CODEX: &str = "codex";
const SOURCE_CLAUDE: &str = "claude-code";
const SOURCE_OPENCODE: &str = "opencode";
const SOURCE_WORKBUDDY: &str = "workbuddy";
const SOURCE_WORKBUDDY_AI: &str = "workbuddy-ai";
const SOURCE_CURSOR: &str = "cursor";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceTokenUsage {
    /// Input that was not served from a prompt cache.
    pub(crate) input_tokens: i64,
    pub(crate) cached_input_tokens: i64,
    pub(crate) cache_write_tokens: i64,
    pub(crate) cache_write_5m_tokens: i64,
    pub(crate) cache_write_1h_tokens: i64,
    pub(crate) cache_write_unknown_tokens: i64,
    pub(crate) output_tokens: i64,
    /// A subset of output tokens. It is not added again to total_tokens.
    pub(crate) reasoning_output_tokens: i64,
    pub(crate) total_tokens: i64,
}

#[derive(Clone, Copy, Debug, Default)]
struct CacheWrites {
    five_minutes: i64,
    one_hour: i64,
    unknown: i64,
}

impl CacheWrites {
    fn unknown(tokens: i64) -> Self {
        Self {
            unknown: tokens,
            ..Self::default()
        }
    }
}

impl DeviceTokenUsage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.cache_write_5m_tokens += other.cache_write_5m_tokens;
        self.cache_write_1h_tokens += other.cache_write_1h_tokens;
        self.cache_write_unknown_tokens += other.cache_write_unknown_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_output_tokens += other.reasoning_output_tokens;
        self.total_tokens += other.total_tokens;
    }

    fn from_parts(
        input: i64,
        cache_read: i64,
        cache_write: i64,
        output: i64,
        reasoning: i64,
        total: Option<i64>,
    ) -> Self {
        Self::from_cache_parts(
            input,
            cache_read,
            CacheWrites::unknown(cache_write),
            output,
            reasoning,
            total,
        )
    }

    fn from_cache_parts(
        input: i64,
        cache_read: i64,
        cache_writes: CacheWrites,
        output: i64,
        reasoning: i64,
        total: Option<i64>,
    ) -> Self {
        let input_tokens = input.max(0);
        let cached_input_tokens = cache_read.max(0);
        let cache_write_5m_tokens = cache_writes.five_minutes.max(0);
        let cache_write_1h_tokens = cache_writes.one_hour.max(0);
        let cache_write_unknown_tokens = cache_writes.unknown.max(0);
        let cache_write_tokens =
            cache_write_5m_tokens + cache_write_1h_tokens + cache_write_unknown_tokens;
        let output_tokens = output.max(0);
        let reasoning_output_tokens = reasoning.clamp(0, output_tokens);
        let computed = input_tokens + cached_input_tokens + cache_write_tokens + output_tokens;
        Self {
            input_tokens,
            cached_input_tokens,
            cache_write_tokens,
            cache_write_5m_tokens,
            cache_write_1h_tokens,
            cache_write_unknown_tokens,
            output_tokens,
            reasoning_output_tokens,
            total_tokens: total.unwrap_or(computed).max(computed),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceUsageDimension {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) usage: DeviceTokenUsage,
    pub(crate) cost: CostSummary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceDailyUsageBucket {
    pub(crate) date: String,
    pub(crate) usage: DeviceTokenUsage,
    pub(crate) cost: CostSummary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceHourlyUsageBucket {
    pub(crate) hour_start_ms: i64,
    pub(crate) usage: DeviceTokenUsage,
    pub(crate) cost: CostSummary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceUsageRangeSummary {
    pub(crate) usage: DeviceTokenUsage,
    pub(crate) cost: CostSummary,
    pub(crate) by_model: Vec<DeviceUsageDimension>,
    pub(crate) by_project: Vec<DeviceUsageDimension>,
    pub(crate) daily_usage: Vec<DeviceDailyUsageBucket>,
    pub(crate) hourly_usage: Vec<DeviceHourlyUsageBucket>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceUsageRanges {
    pub(crate) today: DeviceUsageRangeSummary,
    pub(crate) last24_hours: DeviceUsageRangeSummary,
    pub(crate) last7_days: DeviceUsageRangeSummary,
    pub(crate) last30_days: DeviceUsageRangeSummary,
    pub(crate) total: DeviceUsageRangeSummary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageSourceSummary {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) status: String,
    pub(crate) message: Option<String>,
    pub(crate) source_files: i64,
    pub(crate) indexed_events: i64,
    pub(crate) new_events: Option<i64>,
    pub(crate) skipped_records: Option<i64>,
    pub(crate) last_indexed_at: Option<String>,
    pub(crate) total: DeviceTokenUsage,
    pub(crate) today: DeviceTokenUsage,
    pub(crate) cost: CostSummary,
    pub(crate) today_cost: CostSummary,
    pub(crate) by_model: Vec<DeviceUsageDimension>,
    pub(crate) today_by_model: Vec<DeviceUsageDimension>,
    pub(crate) by_project: Vec<DeviceUsageDimension>,
    /// A provider-specific balance value, never included in Token totals.
    pub(crate) credits: Option<SourceCreditBalance>,
    pub(crate) ranges: DeviceUsageRanges,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceCreditBalance {
    pub(crate) balance: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceUsageSummary {
    pub(crate) generated_at: String,
    pub(crate) price_snapshot_date: String,
    pub(crate) price_catalog: Vec<pricing::PriceCatalogEntry>,
    pub(crate) total: DeviceTokenUsage,
    pub(crate) today: DeviceTokenUsage,
    pub(crate) cost: CostSummary,
    pub(crate) today_cost: CostSummary,
    pub(crate) sources: Vec<UsageSourceSummary>,
    pub(crate) by_model: Vec<DeviceUsageDimension>,
    pub(crate) today_by_model: Vec<DeviceUsageDimension>,
    pub(crate) by_project: Vec<DeviceUsageDimension>,
    pub(crate) warnings: Vec<String>,
    pub(crate) ranges: DeviceUsageRanges,
}

#[derive(Clone, Debug)]
struct UsageEvent {
    identity: String,
    occurred_at_ms: Option<i64>,
    model_label: String,
    project_label: String,
    usage: DeviceTokenUsage,
    cache_metrics_available: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskUsageDetail {
    pub(crate) occurred_at_ms: Option<i64>,
    pub(crate) source: String,
    pub(crate) model_label: String,
    pub(crate) project_label: String,
    /// Neutral label only: raw ids and conversation titles never leave SQLite.
    pub(crate) task_label: String,
    pub(crate) usage: DeviceTokenUsage,
    pub(crate) cache_metrics_available: bool,
    pub(crate) cache_hit_rate: Option<f64>,
    pub(crate) low_cache_hit: bool,
    pub(crate) cost: CostSummary,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageDayDetail {
    pub(crate) date: String,
    pub(crate) tasks: Vec<TaskUsageDetail>,
    pub(crate) low_cache_tasks: i64,
}

#[derive(Clone, Debug, Default)]
struct DimensionAccumulator {
    label: String,
    usage: DeviceTokenUsage,
    cost: CostSummary,
}

impl DimensionAccumulator {
    fn add(&mut self, label: String, usage: DeviceTokenUsage, cost: CostSummary) {
        if self.label.is_empty() {
            self.label = label;
        }
        self.usage.add_assign(usage);
        self.cost.add_assign(&cost);
    }
}

#[derive(Clone, Debug, Default)]
struct RangeAccumulator {
    usage: DeviceTokenUsage,
    cost: CostSummary,
    by_model: HashMap<String, DimensionAccumulator>,
    by_project: HashMap<String, DimensionAccumulator>,
    daily: BTreeMap<NaiveDate, (DeviceTokenUsage, CostSummary)>,
    hourly: BTreeMap<i64, (DeviceTokenUsage, CostSummary)>,
}

impl RangeAccumulator {
    fn add_event(
        &mut self,
        model: &str,
        project: &str,
        usage: DeviceTokenUsage,
        cost: &CostSummary,
        date: Option<NaiveDate>,
        hour_start_ms: Option<i64>,
    ) {
        self.usage.add_assign(usage);
        self.cost.add_assign(cost);
        self.by_model.entry(model.to_owned()).or_default().add(
            model.to_owned(),
            usage,
            cost.clone(),
        );
        self.by_project.entry(project.to_owned()).or_default().add(
            project.to_owned(),
            usage,
            cost.clone(),
        );
        if let Some(date) = date {
            let entry = self.daily.entry(date).or_default();
            entry.0.add_assign(usage);
            entry.1.add_assign(cost);
        }
        if let Some(hour_start_ms) = hour_start_ms {
            let entry = self.hourly.entry(hour_start_ms).or_default();
            entry.0.add_assign(usage);
            entry.1.add_assign(cost);
        }
    }

    fn add_summary(&mut self, source: &UsageSourceSummary, summary: &DeviceUsageRangeSummary) {
        self.usage.add_assign(summary.usage);
        self.cost.add_assign(&summary.cost);
        for dimension in &summary.by_model {
            let key = format!("{}:{}", source.id, dimension.key);
            self.by_model.entry(key).or_default().add(
                format!("{} · {}", source.label, dimension.label),
                dimension.usage,
                dimension.cost.clone(),
            );
        }
        for dimension in &summary.by_project {
            let key = format!("{}:{}", source.id, dimension.key);
            self.by_project.entry(key).or_default().add(
                format!("{} · {}", source.label, dimension.label),
                dimension.usage,
                dimension.cost.clone(),
            );
        }
        for bucket in &summary.daily_usage {
            if let Ok(date) = NaiveDate::parse_from_str(&bucket.date, "%Y-%m-%d") {
                let entry = self.daily.entry(date).or_default();
                entry.0.add_assign(bucket.usage);
                entry.1.add_assign(&bucket.cost);
            }
        }
        for bucket in &summary.hourly_usage {
            let entry = self.hourly.entry(bucket.hour_start_ms).or_default();
            entry.0.add_assign(bucket.usage);
            entry.1.add_assign(&bucket.cost);
        }
    }

    fn finish(self) -> DeviceUsageRangeSummary {
        DeviceUsageRangeSummary {
            usage: self.usage,
            cost: self.cost,
            by_model: dimensions_from_map(self.by_model),
            by_project: dimensions_from_map(self.by_project),
            daily_usage: self
                .daily
                .into_iter()
                .map(|(date, (usage, cost))| DeviceDailyUsageBucket {
                    date: date.format("%Y-%m-%d").to_string(),
                    usage,
                    cost,
                })
                .collect(),
            hourly_usage: self
                .hourly
                .into_iter()
                .map(|(hour_start_ms, (usage, cost))| DeviceHourlyUsageBucket {
                    hour_start_ms,
                    usage,
                    cost,
                })
                .collect(),
        }
    }
}

#[derive(Default)]
struct RangesAccumulator {
    today: RangeAccumulator,
    last24_hours: RangeAccumulator,
    last7_days: RangeAccumulator,
    last30_days: RangeAccumulator,
    total: RangeAccumulator,
}

impl RangesAccumulator {
    fn add_event(
        &mut self,
        model: &str,
        project: &str,
        usage: DeviceTokenUsage,
        cost: &CostSummary,
        occurred_at_ms: Option<i64>,
        today: NaiveDate,
    ) {
        let date = occurred_at_ms.and_then(local_date_from_ms);
        let now_ms = Utc::now().timestamp_millis();
        let hour_start_ms = occurred_at_ms.and_then(|value| recent_hour_start(value, now_ms));
        self.total
            .add_event(model, project, usage, cost, date, hour_start_ms);
        if hour_start_ms.is_some() {
            self.last24_hours
                .add_event(model, project, usage, cost, date, hour_start_ms);
        }
        if let Some(date) = date {
            if date >= today - ChronoDuration::days(29) && date <= today {
                self.last30_days
                    .add_event(model, project, usage, cost, Some(date), hour_start_ms);
            }
            if date >= today - ChronoDuration::days(6) && date <= today {
                self.last7_days
                    .add_event(model, project, usage, cost, Some(date), hour_start_ms);
            }
            if date == today {
                self.today
                    .add_event(model, project, usage, cost, Some(date), hour_start_ms);
            }
        }
    }

    fn add_source(&mut self, source: &UsageSourceSummary) {
        self.today.add_summary(source, &source.ranges.today);
        self.last24_hours
            .add_summary(source, &source.ranges.last24_hours);
        self.last7_days
            .add_summary(source, &source.ranges.last7_days);
        self.last30_days
            .add_summary(source, &source.ranges.last30_days);
        self.total.add_summary(source, &source.ranges.total);
    }

    fn finish(self) -> DeviceUsageRanges {
        DeviceUsageRanges {
            today: self.today.finish(),
            last24_hours: self.last24_hours.finish(),
            last7_days: self.last7_days.finish(),
            last30_days: self.last30_days.finish(),
            total: self.total.finish(),
        }
    }
}

#[derive(Clone, Debug)]
struct SourceState {
    id: &'static str,
    label: &'static str,
    status: &'static str,
    message: Option<String>,
    credits: Option<SourceCreditBalance>,
    scan: Option<SourceScanStats>,
    last_indexed_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
struct SourceScanStats {
    new_events: i64,
    skipped_records: i64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SourceScanProgress {
    pub(crate) source_label: &'static str,
    pub(crate) processed_sources: i64,
    pub(crate) processed_files: i64,
    pub(crate) total_files: i64,
}

impl SourceState {
    fn ready(id: &'static str, label: &'static str) -> Self {
        Self {
            id,
            label,
            status: "ready",
            message: None,
            credits: None,
            scan: None,
            last_indexed_at: None,
        }
    }

    fn not_detected(id: &'static str, label: &'static str, message: &str) -> Self {
        Self {
            id,
            label,
            status: "notDetected",
            message: Some(message.to_owned()),
            credits: None,
            scan: Some(SourceScanStats::default()),
            last_indexed_at: None,
        }
    }

    fn error(id: &'static str, label: &'static str, message: &str) -> Self {
        Self {
            id,
            label,
            status: "error",
            message: Some(message.to_owned()),
            credits: None,
            scan: None,
            last_indexed_at: None,
        }
    }

    fn with_credits(mut self, credits: Option<SourceCreditBalance>) -> Self {
        self.credits = credits;
        self
    }

    fn with_scan(mut self, scan: SourceScanStats) -> Self {
        self.scan = Some(scan);
        self.last_indexed_at = Some(Utc::now().to_rfc3339());
        self
    }
}

pub(crate) struct MultiSourceIndex {
    connection: Connection,
}

impl MultiSourceIndex {
    pub(crate) fn open(path: &Path) -> rusqlite::Result<Self> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS source_files (
              source TEXT NOT NULL,
              file_key BLOB NOT NULL CHECK(length(file_key) = 32),
              size_bytes INTEGER NOT NULL,
              modified_at_ms INTEGER,
              present INTEGER NOT NULL CHECK(present IN (0, 1)),
              PRIMARY KEY(source, file_key)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS source_events (
              event_key BLOB PRIMARY KEY CHECK(length(event_key) = 32),
              source TEXT NOT NULL,
              source_file_key BLOB NOT NULL CHECK(length(source_file_key) = 32),
              occurred_at_ms INTEGER,
              model_label TEXT NOT NULL,
              project_label TEXT NOT NULL,
              input_tokens INTEGER NOT NULL CHECK(input_tokens >= 0),
              cached_input_tokens INTEGER NOT NULL CHECK(cached_input_tokens >= 0),
              cache_write_tokens INTEGER NOT NULL CHECK(cache_write_tokens >= 0),
              cache_write_5m_tokens INTEGER NOT NULL DEFAULT 0 CHECK(cache_write_5m_tokens >= 0),
              cache_write_1h_tokens INTEGER NOT NULL DEFAULT 0 CHECK(cache_write_1h_tokens >= 0),
              cache_write_unknown_tokens INTEGER NOT NULL DEFAULT 0 CHECK(cache_write_unknown_tokens >= 0),
              cache_metrics_available INTEGER NOT NULL DEFAULT 0 CHECK(cache_metrics_available IN (0, 1)),
              output_tokens INTEGER NOT NULL CHECK(output_tokens >= 0),
              reasoning_output_tokens INTEGER NOT NULL CHECK(reasoning_output_tokens >= 0),
              total_tokens INTEGER NOT NULL CHECK(total_tokens >= 0)
            );
            CREATE INDEX IF NOT EXISTS source_events_source ON source_events(source);
            CREATE INDEX IF NOT EXISTS source_events_time ON source_events(occurred_at_ms);
            CREATE INDEX IF NOT EXISTS source_events_file ON source_events(source, source_file_key);

            CREATE TABLE IF NOT EXISTS source_event_files (
              source TEXT NOT NULL,
              source_file_key BLOB NOT NULL CHECK(length(source_file_key) = 32),
              event_key BLOB NOT NULL CHECK(length(event_key) = 32),
              PRIMARY KEY(source, source_file_key, event_key)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS source_event_files_event ON source_event_files(source, event_key);

            CREATE TABLE IF NOT EXISTS source_index_meta (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );
            ",
        )?;
        ensure_source_event_columns(&connection)?;
        migrate_derived_source_index(&mut connection)?;
        Ok(Self { connection })
    }

    pub(crate) fn collect(
        &mut self,
        user_home: &Path,
        codex_local: Result<(LocalUsageSummary, Vec<LocalUsageEvent>, Option<i64>), ()>,
    ) -> DeviceUsageSummary {
        self.collect_with_progress(user_home, codex_local, |_| {})
    }

    pub(crate) fn collect_with_progress<F>(
        &mut self,
        user_home: &Path,
        codex_local: Result<(LocalUsageSummary, Vec<LocalUsageEvent>, Option<i64>), ()>,
        mut on_progress: F,
    ) -> DeviceUsageSummary
    where
        F: FnMut(SourceScanProgress),
    {
        let mut states = Vec::new();
        let codex = match codex_local {
            Ok((summary, events, new_events)) => codex_summary(summary, events, new_events),
            Err(()) => empty_source_summary(SourceState::error(
                SOURCE_CODEX,
                "Codex",
                "Codex 本机日志索引暂不可用。",
            )),
        };
        states.push(self.scan_claude_with_progress(
            &user_home.join(".claude"),
            |processed, total| {
                on_progress(SourceScanProgress {
                    source_label: "Claude Code",
                    processed_sources: 1,
                    processed_files: processed,
                    total_files: total,
                });
            },
        ));
        states.push(
            self.scan_opencode_with_progress(
                &user_home
                    .join(".local")
                    .join("share")
                    .join("opencode")
                    .join("opencode.db"),
                |processed, total| {
                    on_progress(SourceScanProgress {
                        source_label: "OpenCode",
                        processed_sources: 2,
                        processed_files: processed,
                        total_files: total,
                    });
                },
            ),
        );
        states.push(self.scan_workbuddy_with_progress(
            SOURCE_WORKBUDDY,
            "WorkBuddy",
            &user_home.join(".workbuddy"),
            |processed, total| {
                on_progress(SourceScanProgress {
                    source_label: "WorkBuddy",
                    processed_sources: 3,
                    processed_files: processed,
                    total_files: total,
                });
            },
        ));
        states.push(self.scan_workbuddy_with_progress(
            SOURCE_WORKBUDDY_AI,
            "WorkBuddy AI",
            &user_home.join(".workbuddy-ai"),
            |processed, total| {
                on_progress(SourceScanProgress {
                    source_label: "WorkBuddy AI",
                    processed_sources: 4,
                    processed_files: processed,
                    total_files: total,
                });
            },
        ));
        on_progress(SourceScanProgress {
            source_label: "Cursor",
            processed_sources: 5,
            processed_files: 0,
            total_files: 0,
        });
        states.push(SourceState {
            id: SOURCE_CURSOR,
            label: "Cursor",
            status: "unavailable",
            message: Some("个人账户暂无可信的本机精确 Token 数据来源。".to_owned()),
            credits: None,
            scan: None,
            last_indexed_at: None,
        });

        let mut sources = vec![codex];
        for state in states {
            sources.push(self.aggregate_source(self.with_persisted_last_success(state)));
        }

        let mut ranges = RangesAccumulator::default();
        for source in &sources {
            ranges.add_source(source);
        }
        let ranges = ranges.finish();

        DeviceUsageSummary {
            generated_at: Utc::now().to_rfc3339(),
            price_snapshot_date: pricing::PRICE_SNAPSHOT_DATE.to_owned(),
            price_catalog: pricing::catalog(),
            total: ranges.total.usage,
            today: ranges.today.usage,
            cost: ranges.total.cost.clone(),
            today_cost: ranges.today.cost.clone(),
            by_model: ranges.total.by_model.clone(),
            today_by_model: ranges.today.by_model.clone(),
            by_project: ranges.total.by_project.clone(),
            sources,
            warnings: Vec::new(),
            ranges,
        }
    }

    fn scan_claude_with_progress<F>(
        &mut self,
        claude_home: &Path,
        mut on_progress: F,
    ) -> SourceState
    where
        F: FnMut(i64, i64),
    {
        let project_root = claude_home.join("projects");
        if !project_root.is_dir() {
            on_progress(0, 0);
            return SourceState::not_detected(
                SOURCE_CLAUDE,
                "Claude Code",
                "未检测到 Claude Code 本机会话目录。",
            );
        }
        let files = collect_files(&project_root, |path| extension_is(path, "jsonl"));
        match self.scan_files(SOURCE_CLAUDE, &files, parse_claude_file, &mut on_progress) {
            Err(()) => SourceState::error(
                SOURCE_CLAUDE,
                "Claude Code",
                "Claude Code 本机索引暂不可写入。",
            ),
            Ok(scan) => SourceState::ready(SOURCE_CLAUDE, "Claude Code").with_scan(scan),
        }
    }

    fn scan_workbuddy_with_progress<F>(
        &mut self,
        id: &'static str,
        label: &'static str,
        root: &Path,
        mut on_progress: F,
    ) -> SourceState
    where
        F: FnMut(i64, i64),
    {
        let trace_root = root.join("traces");
        if !trace_root.is_dir() {
            on_progress(0, 0);
            return SourceState::not_detected(id, label, "未检测到本机 trace 用量目录。");
        }
        let labels = read_workbuddy_session_labels(&root.join("workbuddy.db"));
        let credits = read_workbuddy_credit_balance(&root.join("workbuddy.db"));
        let files = collect_files(&trace_root, is_workbuddy_trace);
        match self.scan_files(
            id,
            &files,
            |path, contents| parse_workbuddy_trace(path, contents, &labels),
            &mut on_progress,
        ) {
            Err(()) => SourceState::error(id, label, "本机 trace 用量索引暂不可写入。"),
            Ok(scan) => SourceState::ready(id, label)
                .with_credits(credits)
                .with_scan(scan),
        }
    }

    fn scan_opencode_with_progress<F>(
        &mut self,
        database_path: &Path,
        mut on_progress: F,
    ) -> SourceState
    where
        F: FnMut(i64, i64),
    {
        if !database_path.is_file() {
            on_progress(0, 0);
            return SourceState::not_detected(
                SOURCE_OPENCODE,
                "OpenCode",
                "未检测到 OpenCode 桌面版本地数据库。",
            );
        }
        on_progress(0, 1);
        let mut scan = SourceScanStats::default();
        let file_key = hash_bytes(database_path.to_string_lossy().as_bytes());
        let (size_bytes, modified_at_ms) = database_stamp(database_path);
        let changed = self.file_changed(SOURCE_OPENCODE, &file_key, size_bytes, modified_at_ms);
        if changed {
            let read = read_opencode_events(database_path);
            let parsed = match read {
                Ok(events) => events,
                Err(()) => {
                    return SourceState::error(
                        SOURCE_OPENCODE,
                        "OpenCode",
                        "OpenCode 数据库正忙或格式暂不可读取。",
                    )
                }
            };
            let transaction = match self.connection.transaction() {
                Ok(transaction) => transaction,
                Err(_) => {
                    return SourceState::error(
                        SOURCE_OPENCODE,
                        "OpenCode",
                        "本机用量索引暂不可写入。",
                    )
                }
            };
            if transaction
                .execute(
                    "DELETE FROM source_event_files WHERE source = ?1 AND source_file_key = ?2",
                    params![SOURCE_OPENCODE, file_key.as_slice()],
                )
                .is_err()
            {
                return SourceState::error(SOURCE_OPENCODE, "OpenCode", "本机用量索引暂不可写入。");
            }
            scan.skipped_records += parsed.skipped_records;
            for event in parsed.events {
                match insert_event(&transaction, SOURCE_OPENCODE, &file_key, event) {
                    Ok(true) => scan.new_events += 1,
                    Ok(false) => {}
                    Err(_) => {
                        return SourceState::error(
                            SOURCE_OPENCODE,
                            "OpenCode",
                            "本机用量索引暂不可写入。",
                        )
                    }
                };
            }
            if upsert_source_file(
                &transaction,
                SOURCE_OPENCODE,
                &file_key,
                size_bytes,
                modified_at_ms,
            )
            .is_err()
                || prune_orphan_events(&transaction, SOURCE_OPENCODE).is_err()
            {
                return SourceState::error(SOURCE_OPENCODE, "OpenCode", "本机用量索引暂不可写入。");
            }
            if transaction.commit().is_err() {
                return SourceState::error(SOURCE_OPENCODE, "OpenCode", "本机用量索引暂不可写入。");
            }
        } else {
            let _ = self.connection.execute(
                "UPDATE source_files SET present = 1 WHERE source = ?1 AND file_key = ?2",
                params![SOURCE_OPENCODE, file_key.as_slice()],
            );
        }
        on_progress(1, 1);
        SourceState::ready(SOURCE_OPENCODE, "OpenCode").with_scan(scan)
    }

    #[cfg(test)]
    fn scan_claude(&mut self, claude_home: &Path) -> SourceState {
        self.scan_claude_with_progress(claude_home, |_, _| {})
    }

    #[cfg(test)]
    fn scan_workbuddy(
        &mut self,
        id: &'static str,
        label: &'static str,
        root: &Path,
    ) -> SourceState {
        self.scan_workbuddy_with_progress(id, label, root, |_, _| {})
    }

    #[cfg(test)]
    fn scan_opencode(&mut self, database_path: &Path) -> SourceState {
        self.scan_opencode_with_progress(database_path, |_, _| {})
    }

    fn scan_files<F, P>(
        &mut self,
        source: &str,
        files: &[PathBuf],
        parser: F,
        mut on_progress: P,
    ) -> Result<SourceScanStats, ()>
    where
        F: Fn(&Path, &str) -> ParsedUsageEvents,
        P: FnMut(i64, i64),
    {
        let total_files = i64::try_from(files.len()).unwrap_or(i64::MAX);
        let mut scan = SourceScanStats::default();
        on_progress(0, total_files);
        self.connection
            .execute(
                "UPDATE source_files SET present = 0 WHERE source = ?1",
                [source],
            )
            .map_err(|_| ())?;
        for (index, path) in files.iter().enumerate() {
            let file_key = hash_bytes(path.to_string_lossy().as_bytes());
            let contents = match fs::read_to_string(path) {
                Ok(contents) => contents,
                Err(_) => {
                    scan.skipped_records += 1;
                    let _ = self.connection.execute(
                        "UPDATE source_files SET present = 1 WHERE source = ?1 AND file_key = ?2",
                        params![source, file_key.as_slice()],
                    );
                    on_progress(i64::try_from(index + 1).unwrap_or(i64::MAX), total_files);
                    continue;
                }
            };
            let (size_bytes, modified_at_ms) = file_stamp(path);
            let changed = self.file_changed(source, &file_key, size_bytes, modified_at_ms);
            let transaction = match self.connection.transaction() {
                Ok(transaction) => transaction,
                Err(_) => return Err(()),
            };
            if changed {
                transaction
                    .execute(
                        "DELETE FROM source_event_files WHERE source = ?1 AND source_file_key = ?2",
                        params![source, file_key.as_slice()],
                    )
                    .map_err(|_| ())?;
                let parsed = parser(path, &contents);
                scan.skipped_records += parsed.skipped_records;
                for event in parsed.events {
                    if insert_event(&transaction, source, &file_key, event).map_err(|_| ())? {
                        scan.new_events += 1;
                    }
                }
            }
            upsert_source_file(&transaction, source, &file_key, size_bytes, modified_at_ms)
                .map_err(|_| ())?;
            transaction.commit().map_err(|_| ())?;
            on_progress(i64::try_from(index + 1).unwrap_or(i64::MAX), total_files);
        }
        self.connection.execute(
            "DELETE FROM source_event_files WHERE source = ?1 AND source_file_key IN (SELECT file_key FROM source_files WHERE source = ?1 AND present = 0)",
            [source],
        ).map_err(|_| ())?;
        self.connection
            .execute(
                "DELETE FROM source_files WHERE source = ?1 AND present = 0",
                [source],
            )
            .map_err(|_| ())?;
        let transaction = self.connection.transaction().map_err(|_| ())?;
        prune_orphan_events(&transaction, source).map_err(|_| ())?;
        transaction.commit().map_err(|_| ())?;
        Ok(scan)
    }

    fn file_changed(
        &self,
        source: &str,
        file_key: &[u8; 32],
        size_bytes: i64,
        modified_at_ms: Option<i64>,
    ) -> bool {
        let previous = self.connection.query_row(
            "SELECT size_bytes, modified_at_ms FROM source_files WHERE source = ?1 AND file_key = ?2",
            params![source, file_key.as_slice()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
        ).optional();
        !matches!(previous, Ok(Some((size, modified))) if size == size_bytes && modified == modified_at_ms)
    }

    fn with_persisted_last_success(&self, mut state: SourceState) -> SourceState {
        let key = format!("last-success:{}", state.id);
        if state.status == "ready" {
            if let Some(value) = state.last_indexed_at.as_deref() {
                let _ = self.connection.execute(
                    "INSERT INTO source_index_meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                );
            }
        } else {
            state.last_indexed_at = self
                .connection
                .query_row(
                    "SELECT value FROM source_index_meta WHERE key = ?1",
                    [key],
                    |row| row.get(0),
                )
                .optional()
                .ok()
                .flatten();
        }
        state
    }

    fn aggregate_source(&self, state: SourceState) -> UsageSourceSummary {
        if state.status == "unavailable" {
            return empty_source_summary(state);
        }
        let source_files = self
            .connection
            .query_row(
                "SELECT count(*) FROM source_files WHERE source = ?1",
                [state.id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let mut ranges = RangesAccumulator::default();
        let mut indexed_events = 0_i64;
        let today_date = Local::now().date_naive();
        let events = self.connection.prepare(
            "SELECT occurred_at_ms, model_label, project_label, input_tokens, cached_input_tokens, cache_write_tokens, cache_write_5m_tokens, cache_write_1h_tokens, cache_write_unknown_tokens, output_tokens, reasoning_output_tokens, total_tokens FROM source_events WHERE source = ?1",
        );
        if let Ok(mut statement) = events {
            if let Ok(rows) = statement.query_map([state.id], |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    DeviceTokenUsage {
                        input_tokens: row.get(3)?,
                        cached_input_tokens: row.get(4)?,
                        cache_write_tokens: row.get(5)?,
                        cache_write_5m_tokens: row.get(6)?,
                        cache_write_1h_tokens: row.get(7)?,
                        cache_write_unknown_tokens: row.get(8)?,
                        output_tokens: row.get(9)?,
                        reasoning_output_tokens: row.get(10)?,
                        total_tokens: row.get(11)?,
                    },
                ))
            }) {
                for row in rows.flatten() {
                    indexed_events += 1;
                    let event_cost = cost_for(&row.1, row.3, row.0);
                    ranges.add_event(&row.1, &row.2, row.3, &event_cost, row.0, today_date);
                }
            }
        }
        let ranges = ranges.finish();
        UsageSourceSummary {
            id: state.id.to_owned(),
            label: state.label.to_owned(),
            status: state.status.to_owned(),
            message: state.message,
            source_files,
            indexed_events,
            new_events: state.scan.map(|scan| scan.new_events),
            skipped_records: state.scan.map(|scan| scan.skipped_records),
            last_indexed_at: state.last_indexed_at,
            total: ranges.total.usage,
            today: ranges.today.usage,
            cost: ranges.total.cost.clone(),
            today_cost: ranges.today.cost.clone(),
            by_model: ranges.total.by_model.clone(),
            today_by_model: ranges.today.by_model.clone(),
            by_project: ranges.total.by_project.clone(),
            credits: state.credits,
            ranges,
        }
    }

    pub(crate) fn day_tasks(&self, date: &str) -> Result<Vec<TaskUsageDetail>, ()> {
        let Some((start, end)) = local_day_bounds(date) else {
            return Err(());
        };
        let mut statement = self.connection.prepare(
            "SELECT source, occurred_at_ms, model_label, project_label, input_tokens, cached_input_tokens, cache_write_tokens, cache_write_5m_tokens, cache_write_1h_tokens, cache_write_unknown_tokens, cache_metrics_available, output_tokens, reasoning_output_tokens, total_tokens
             FROM source_events
             WHERE occurred_at_ms >= ?1 AND occurred_at_ms < ?2
             ORDER BY occurred_at_ms DESC",
        ).map_err(|_| ())?;
        let rows = statement
            .query_map(params![start, end], |row| {
                let source: String = row.get(0)?;
                let occurred_at_ms: Option<i64> = row.get(1)?;
                let model_label: String = row.get(2)?;
                let project_label: String = row.get(3)?;
                let usage = DeviceTokenUsage {
                    input_tokens: row.get(4)?,
                    cached_input_tokens: row.get(5)?,
                    cache_write_tokens: row.get(6)?,
                    cache_write_5m_tokens: row.get(7)?,
                    cache_write_1h_tokens: row.get(8)?,
                    cache_write_unknown_tokens: row.get(9)?,
                    output_tokens: row.get(11)?,
                    reasoning_output_tokens: row.get(12)?,
                    total_tokens: row.get(13)?,
                };
                let cache_metrics_available = row.get::<_, i64>(10)? != 0;
                Ok(task_detail(
                    occurred_at_ms,
                    source,
                    model_label,
                    project_label,
                    usage,
                    cache_metrics_available,
                ))
            })
            .map_err(|_| ())?;
        Ok(rows.flatten().collect())
    }
}

pub(crate) fn build_day_detail(date: String, mut tasks: Vec<TaskUsageDetail>) -> UsageDayDetail {
    tasks.sort_by_key(|task| std::cmp::Reverse(task.occurred_at_ms.unwrap_or(i64::MIN)));
    let low_cache_tasks = tasks.iter().filter(|task| task.low_cache_hit).count() as i64;
    UsageDayDetail {
        date,
        tasks,
        low_cache_tasks,
    }
}

pub(crate) fn codex_day_tasks(events: Vec<LocalUsageEvent>) -> Vec<TaskUsageDetail> {
    events
        .into_iter()
        .map(|event| {
            let usage = DeviceTokenUsage::from_parts(
                event.usage.input_tokens - event.usage.cached_input_tokens,
                event.usage.cached_input_tokens,
                0,
                event.usage.output_tokens,
                event.usage.reasoning_output_tokens,
                Some(event.usage.total_tokens),
            );
            task_detail(
                event.occurred_at_ms,
                SOURCE_CODEX.to_owned(),
                event.model_label,
                event.project_label,
                usage,
                event.cache_metrics_available,
            )
        })
        .collect()
}

fn task_detail(
    occurred_at_ms: Option<i64>,
    source: String,
    model_label: String,
    project_label: String,
    usage: DeviceTokenUsage,
    cache_metrics_available: bool,
) -> TaskUsageDetail {
    let denominator = usage.input_tokens + usage.cached_input_tokens + usage.cache_write_tokens;
    let cache_hit_rate = if cache_metrics_available && denominator > 0 {
        Some(usage.cached_input_tokens as f64 / denominator as f64)
    } else {
        None
    };
    let task_label = occurred_at_ms
        .and_then(|time| Local.timestamp_millis_opt(time).single())
        .map(|time| format!("本机任务 · {}", time.format("%H:%M")))
        .unwrap_or_else(|| "本机任务".to_owned());
    let cost = cost_for(&model_label, usage, occurred_at_ms);
    TaskUsageDetail {
        occurred_at_ms,
        source: source_label(&source).to_owned(),
        model_label,
        project_label,
        task_label,
        usage,
        cache_metrics_available,
        low_cache_hit: cache_hit_rate.is_some_and(|rate| rate < 0.9),
        cache_hit_rate,
        cost,
    }
}

fn local_day_bounds(date: &str) -> Option<(i64, i64)> {
    let date = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let start_naive = date.and_hms_opt(0, 0, 0)?;
    let end_naive = date.succ_opt()?.and_hms_opt(0, 0, 0)?;
    let start = Local.from_local_datetime(&start_naive).earliest()?;
    let end = Local.from_local_datetime(&end_naive).latest()?;
    Some((start.timestamp_millis(), end.timestamp_millis()))
}

fn source_label(source: &str) -> &str {
    match source {
        SOURCE_CODEX => "Codex",
        SOURCE_CLAUDE => "Claude Code",
        SOURCE_OPENCODE => "OpenCode",
        SOURCE_WORKBUDDY => "WorkBuddy",
        SOURCE_WORKBUDDY_AI => "WorkBuddy AI",
        SOURCE_CURSOR => "Cursor",
        _ => "本机来源",
    }
}

fn codex_summary(
    summary: LocalUsageSummary,
    events: Vec<LocalUsageEvent>,
    new_events: Option<i64>,
) -> UsageSourceSummary {
    let convert = |usage: crate::index::TokenUsage| {
        DeviceTokenUsage::from_parts(
            usage.input_tokens - usage.cached_input_tokens,
            usage.cached_input_tokens,
            0,
            usage.output_tokens,
            usage.reasoning_output_tokens,
            Some(usage.total_tokens),
        )
    };
    let mut ranges = RangesAccumulator::default();
    let today_date = Local::now().date_naive();
    for event in events {
        let usage = convert(event.usage);
        let cost = cost_for(&event.model_label, usage, event.occurred_at_ms);
        ranges.add_event(
            &event.model_label,
            &event.project_label,
            usage,
            &cost,
            event.occurred_at_ms,
            today_date,
        );
    }
    let ranges = ranges.finish();
    UsageSourceSummary {
        id: SOURCE_CODEX.to_owned(),
        label: "Codex".to_owned(),
        status: "ready".to_owned(),
        message: None,
        source_files: summary.source_files,
        indexed_events: summary.indexed_events,
        new_events,
        skipped_records: Some(summary.skipped_events),
        last_indexed_at: Some(Utc::now().to_rfc3339()),
        total: ranges.total.usage,
        today: ranges.today.usage,
        cost: ranges.total.cost.clone(),
        today_cost: ranges.today.cost.clone(),
        by_model: ranges.total.by_model.clone(),
        today_by_model: ranges.today.by_model.clone(),
        credits: None,
        by_project: ranges.total.by_project.clone(),
        ranges,
    }
}

fn empty_source_summary(state: SourceState) -> UsageSourceSummary {
    let ranges = RangesAccumulator::default().finish();
    UsageSourceSummary {
        id: state.id.to_owned(),
        label: state.label.to_owned(),
        status: state.status.to_owned(),
        message: state.message,
        source_files: 0,
        indexed_events: 0,
        new_events: state.scan.map(|scan| scan.new_events),
        skipped_records: state.scan.map(|scan| scan.skipped_records),
        last_indexed_at: state.last_indexed_at,
        total: DeviceTokenUsage::default(),
        today: DeviceTokenUsage::default(),
        cost: CostSummary::default(),
        today_cost: CostSummary::default(),
        by_model: Vec::new(),
        today_by_model: Vec::new(),
        by_project: Vec::new(),
        credits: state.credits,
        ranges,
    }
}

#[derive(Debug, Default)]
struct ParsedUsageEvents {
    events: Vec<UsageEvent>,
    skipped_records: i64,
}

impl std::ops::Deref for ParsedUsageEvents {
    type Target = [UsageEvent];

    fn deref(&self) -> &Self::Target {
        &self.events
    }
}

fn parse_claude_file(_path: &Path, contents: &str) -> ParsedUsageEvents {
    let complete = contents.ends_with('\n');
    let lines: Vec<_> = contents.lines().collect();
    let line_count = lines.len();
    let mut parsed = ParsedUsageEvents::default();
    for (index, line) in lines.into_iter().enumerate() {
        if !complete && index + 1 == line_count {
            continue;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                if !line.trim().is_empty() {
                    parsed.skipped_records += 1;
                }
                continue;
            }
        };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let (Some(input), Some(output)) = (
            number(usage.get("input_tokens")),
            number(usage.get("output_tokens")),
        ) else {
            parsed.skipped_records += 1;
            continue;
        };
        let cache_read = number(usage.get("cache_read_input_tokens")).unwrap_or(0);
        let cache_write_5m = number(usage.pointer("/cache_creation/ephemeral_5m_input_tokens"));
        let cache_write_1h = number(usage.pointer("/cache_creation/ephemeral_1h_input_tokens"));
        let cache_write_unknown = if cache_write_5m.is_some() || cache_write_1h.is_some() {
            0
        } else {
            number(usage.get("cache_creation_input_tokens")).unwrap_or(0)
        };
        let cache_metrics_available = usage.get("cache_read_input_tokens").is_some()
            || usage.get("cache_creation_input_tokens").is_some()
            || cache_write_5m.is_some()
            || cache_write_1h.is_some();
        let session = text(value.get("sessionId"))
            .or_else(|| text(value.get("session_id")))
            .unwrap_or("unknown");
        let message_id = text(value.get("uuid"))
            .or_else(|| text(message.get("id")))
            .unwrap_or("unknown");
        parsed.events.push(UsageEvent {
            identity: format!("{session}:{message_id}"),
            occurred_at_ms: timestamp(value.get("timestamp")),
            model_label: text(message.get("model")).unwrap_or("未知模型").to_owned(),
            project_label: project_label(text(value.get("cwd")).unwrap_or("")),
            usage: DeviceTokenUsage::from_cache_parts(
                input,
                cache_read,
                CacheWrites {
                    five_minutes: cache_write_5m.unwrap_or(0),
                    one_hour: cache_write_1h.unwrap_or(0),
                    unknown: cache_write_unknown,
                },
                output,
                0,
                None,
            ),
            cache_metrics_available,
        });
    }
    parsed
}

fn parse_workbuddy_trace(
    path: &Path,
    contents: &str,
    session_labels: &HashMap<String, String>,
) -> ParsedUsageEvents {
    let value: Value = match serde_json::from_str(contents) {
        Ok(value) => value,
        Err(_) => {
            return ParsedUsageEvents {
                events: Vec::new(),
                skipped_records: 1,
            }
        }
    };
    let raw = value.pointer("/providerData/rawUsage");
    let normalized = value.pointer("/providerData/usage");
    let message_usage = value.pointer("/message/usage");
    let trace = value.get("trace");
    let usage = raw
        .and_then(workbuddy_raw_usage)
        .or_else(|| normalized.and_then(workbuddy_normalized_usage))
        .or_else(|| message_usage.and_then(workbuddy_message_usage))
        .or_else(|| trace.and_then(workbuddy_trace_summary_usage));
    let Some(usage) = usage else {
        return ParsedUsageEvents::default();
    };
    let session = trace
        .and_then(|trace| text(trace.get("sessionId")))
        .or_else(|| text(value.get("sessionId")))
        .unwrap_or("unknown");
    let model = text(value.pointer("/providerData/requestModelName"))
        .or_else(|| text(value.pointer("/providerData/requestModelId")))
        .or_else(|| text(value.pointer("/providerData/model")))
        .or_else(|| trace.and_then(workbuddy_trace_model_label))
        .unwrap_or("未知模型");
    let identity = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("trace")
        .to_owned();
    ParsedUsageEvents {
        events: vec![UsageEvent {
            identity,
            occurred_at_ms: trace
                .and_then(|trace| timestamp(trace.get("startedAt")))
                .or_else(|| timestamp(value.get("timestamp"))),
            model_label: model.to_owned(),
            project_label: session_labels
                .get(session)
                .cloned()
                .unwrap_or_else(|| "未知项目".to_owned()),
            usage,
            cache_metrics_available: workbuddy_cache_metrics_available(
                raw,
                normalized,
                message_usage,
                trace,
            ),
        }],
        skipped_records: 0,
    }
}

fn workbuddy_trace_summary_usage(trace: &Value) -> Option<DeviceTokenUsage> {
    let total = number(trace.get("totalTokens"))?;
    let input = number(trace.pointer("/modelInfo/totalInputTokens")).unwrap_or(0);
    let cached = number(trace.pointer("/modelInfo/totalCachedTokens")).unwrap_or(0);
    let output = number(trace.pointer("/modelInfo/totalOutputTokens")).unwrap_or(0);
    Some(DeviceTokenUsage::from_parts(
        input - cached,
        cached,
        0,
        output,
        0,
        Some(total),
    ))
}

fn workbuddy_trace_model_label(trace: &Value) -> Option<&str> {
    trace
        .pointer("/modelInfo/models/0")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn workbuddy_cache_metrics_available(
    raw: Option<&Value>,
    normalized: Option<&Value>,
    message_usage: Option<&Value>,
    trace: Option<&Value>,
) -> bool {
    raw.is_some_and(|value| {
        value.get("cache_read_input_tokens").is_some()
            || value.get("prompt_cache_hit_tokens").is_some()
            || value.get("cache_creation_input_tokens").is_some()
            || value.get("prompt_cache_write_tokens").is_some()
    }) || normalized
        .is_some_and(|value| value.pointer("/inputTokensDetails/cached_tokens").is_some())
        || message_usage.is_some_and(|value| value.get("cache_read_input_tokens").is_some())
        || trace.is_some_and(|value| value.pointer("/modelInfo/totalCachedTokens").is_some())
}

fn workbuddy_raw_usage(value: &Value) -> Option<DeviceTokenUsage> {
    let prompt = number(value.get("prompt_tokens"))?;
    let cache_read = number(value.get("cache_read_input_tokens"))
        .or_else(|| number(value.get("prompt_cache_hit_tokens")))
        .unwrap_or(0);
    let cache_write = number(value.get("cache_creation_input_tokens"))
        .or_else(|| number(value.get("prompt_cache_write_tokens")))
        .unwrap_or(0);
    let output = number(value.get("completion_tokens"))?;
    let reasoning = number(value.pointer("/completion_tokens_details/reasoning_tokens"))
        .or_else(|| number(value.get("completion_thinking_tokens")))
        .unwrap_or(0);
    Some(DeviceTokenUsage::from_parts(
        prompt - cache_read - cache_write,
        cache_read,
        cache_write,
        output,
        reasoning,
        number(value.get("total_tokens")),
    ))
}

fn workbuddy_normalized_usage(value: &Value) -> Option<DeviceTokenUsage> {
    let input = number(value.get("inputTokens"))?;
    let cache_read = number(value.pointer("/inputTokensDetails/cached_tokens")).unwrap_or(0);
    let output = number(value.get("outputTokens"))?;
    let reasoning = number(value.pointer("/outputTokensDetails/reasoning_tokens")).unwrap_or(0);
    Some(DeviceTokenUsage::from_parts(
        input - cache_read,
        cache_read,
        0,
        output,
        reasoning,
        number(value.get("totalTokens")),
    ))
}

fn workbuddy_message_usage(value: &Value) -> Option<DeviceTokenUsage> {
    let input = number(value.get("input_tokens"))?;
    let cache_read = number(value.get("cache_read_input_tokens")).unwrap_or(0);
    let output = number(value.get("output_tokens"))?;
    Some(DeviceTokenUsage::from_parts(
        input - cache_read,
        cache_read,
        0,
        output,
        0,
        number(value.get("total_tokens")),
    ))
}

fn read_opencode_events(path: &Path) -> Result<ParsedUsageEvents, ()> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| ())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|_| ())?;
    let mut statement = connection.prepare(
        "SELECT part.id, part.time_updated, part.data, session.directory, session.model
         FROM part LEFT JOIN session ON session.id = part.session_id
         WHERE json_extract(part.data, '$.type') = 'step-finish' AND json_type(part.data, '$.tokens') IS NOT NULL",
    ).map_err(|_| ())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|_| ())?;
    let mut parsed = ParsedUsageEvents::default();
    for row in rows {
        let row = match row {
            Ok(row) => row,
            Err(_) => {
                parsed.skipped_records += 1;
                continue;
            }
        };
        let value: Value = match serde_json::from_str(&row.2) {
            Ok(value) => value,
            Err(_) => {
                parsed.skipped_records += 1;
                continue;
            }
        };
        let tokens = match value.get("tokens") {
            Some(value) => value,
            None => {
                parsed.skipped_records += 1;
                continue;
            }
        };
        let input = number(tokens.get("input")).unwrap_or(0);
        let cache_read = number(tokens.pointer("/cache/read")).unwrap_or(0);
        let cache_write = number(tokens.pointer("/cache/write")).unwrap_or(0);
        let output = number(tokens.get("output")).unwrap_or(0);
        let reasoning = number(tokens.get("reasoning")).unwrap_or(0);
        parsed.events.push(UsageEvent {
            identity: row.0,
            occurred_at_ms: Some(row.1),
            model_label: opencode_model_label(row.4.as_deref()),
            project_label: project_label(row.3.as_deref().unwrap_or("")),
            usage: DeviceTokenUsage::from_parts(
                input,
                cache_read,
                cache_write,
                output,
                reasoning,
                number(tokens.get("total")),
            ),
            cache_metrics_available: tokens.get("cache").is_some(),
        });
    }
    Ok(parsed)
}

fn opencode_model_label(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "未知模型".to_owned();
    };
    serde_json::from_str::<Value>(value)
        .ok()
        .and_then(|model| {
            text(model.get("modelID"))
                .or_else(|| text(model.get("id")))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| value.to_owned())
}

fn read_workbuddy_session_labels(path: &Path) -> HashMap<String, String> {
    let Ok(connection) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return HashMap::new();
    };
    let Ok(mut statement) =
        connection.prepare("SELECT id, cwd FROM sessions WHERE deleted_at IS NULL")
    else {
        return HashMap::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    }) else {
        return HashMap::new();
    };
    rows.flatten()
        .map(|(id, cwd)| (id, project_label(cwd.as_deref().unwrap_or(""))))
        .collect()
}

fn read_workbuddy_credit_balance(path: &Path) -> Option<SourceCreditBalance> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let mut statement = connection
        .prepare(
            "SELECT credit_json FROM session_usage
             WHERE credit_json IS NOT NULL
             ORDER BY updated_at DESC",
        )
        .ok()?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .ok()?;
    for json in rows.flatten() {
        if let Some(balance) = serde_json::from_str::<Value>(&json)
            .ok()
            .and_then(credit_balance_from_value)
        {
            return Some(SourceCreditBalance { balance });
        }
    }
    None
}

fn credit_balance_from_value(value: Value) -> Option<f64> {
    fn find(value: &Value, depth: u8) -> Option<f64> {
        if depth > 3 {
            return None;
        }
        let object = value.as_object()?;
        for key in [
            "balance",
            "remainingCredit",
            "remainingCredits",
            "availableCredit",
            "availableCredits",
        ] {
            if let Some(balance) = object
                .get(key)
                .and_then(Value::as_f64)
                .filter(|balance| balance.is_finite() && *balance >= 0.0)
            {
                return Some(balance);
            }
        }
        object.values().find_map(|value| find(value, depth + 1))
    }
    find(&value, 0)
}

fn insert_event(
    transaction: &rusqlite::Transaction<'_>,
    source: &str,
    file_key: &[u8; 32],
    event: UsageEvent,
) -> rusqlite::Result<bool> {
    let event_key = hash_bytes(format!("{source}\0{}", event.identity).as_bytes());
    let existed = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM source_events WHERE source = ?1 AND event_key = ?2)",
        params![source, event_key.as_slice()],
        |row| row.get::<_, bool>(0),
    )?;
    transaction.execute(
        "INSERT OR REPLACE INTO source_events(event_key, source, source_file_key, occurred_at_ms, model_label, project_label, input_tokens, cached_input_tokens, cache_write_tokens, cache_write_5m_tokens, cache_write_1h_tokens, cache_write_unknown_tokens, cache_metrics_available, output_tokens, reasoning_output_tokens, total_tokens)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![event_key.as_slice(), source, file_key.as_slice(), event.occurred_at_ms, event.model_label, event.project_label, event.usage.input_tokens, event.usage.cached_input_tokens, event.usage.cache_write_tokens, event.usage.cache_write_5m_tokens, event.usage.cache_write_1h_tokens, event.usage.cache_write_unknown_tokens, i64::from(event.cache_metrics_available), event.usage.output_tokens, event.usage.reasoning_output_tokens, event.usage.total_tokens],
    )?;
    transaction.execute(
        "INSERT OR IGNORE INTO source_event_files(source, source_file_key, event_key) VALUES(?1, ?2, ?3)",
        params![source, file_key.as_slice(), event_key.as_slice()],
    )?;
    Ok(!existed)
}

fn prune_orphan_events(
    transaction: &rusqlite::Transaction<'_>,
    source: &str,
) -> rusqlite::Result<()> {
    transaction.execute(
        "DELETE FROM source_events AS event
         WHERE event.source = ?1
           AND NOT EXISTS (
             SELECT 1 FROM source_event_files AS mapping
             WHERE mapping.source = event.source AND mapping.event_key = event.event_key
           )",
        [source],
    )?;
    Ok(())
}

fn upsert_source_file(
    transaction: &rusqlite::Transaction<'_>,
    source: &str,
    file_key: &[u8; 32],
    size_bytes: i64,
    modified_at_ms: Option<i64>,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO source_files(source, file_key, size_bytes, modified_at_ms, present) VALUES(?1, ?2, ?3, ?4, 1)
         ON CONFLICT(source, file_key) DO UPDATE SET size_bytes = excluded.size_bytes, modified_at_ms = excluded.modified_at_ms, present = 1",
        params![source, file_key.as_slice(), size_bytes, modified_at_ms],
    )?;
    Ok(())
}

fn dimensions_from_map(values: HashMap<String, DimensionAccumulator>) -> Vec<DeviceUsageDimension> {
    let mut dimensions: Vec<_> = values
        .into_iter()
        .map(|(key, value)| DeviceUsageDimension {
            label: value.label,
            key,
            usage: value.usage,
            cost: value.cost,
        })
        .collect();
    dimensions.sort_by_key(|dimension| std::cmp::Reverse(dimension.usage.total_tokens));
    dimensions
}

fn price_usage(usage: DeviceTokenUsage) -> PriceUsage {
    PriceUsage {
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_write_5m_tokens: usage.cache_write_5m_tokens,
        cache_write_1h_tokens: usage.cache_write_1h_tokens,
        cache_write_unknown_tokens: usage.cache_write_unknown_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
    }
}

fn cost_for(
    model_label: &str,
    usage: DeviceTokenUsage,
    occurred_at_ms: Option<i64>,
) -> CostSummary {
    pricing::estimate(model_label, price_usage(usage), occurred_at_ms)
}

/// Advances only the app's derived source index.  A full re-read is safer than
/// trying to reinterpret old generic cache-write counters as 5m/1h values.
fn migrate_derived_source_index(connection: &mut Connection) -> rusqlite::Result<()> {
    let current: Option<String> = connection
        .query_row(
            "SELECT value FROM source_index_meta WHERE key = 'schema-version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if current
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        == Some(MULTI_SOURCE_SCHEMA_VERSION)
    {
        return Ok(());
    }
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM source_event_files", [])?;
    transaction.execute("DELETE FROM source_events", [])?;
    transaction.execute("DELETE FROM source_files", [])?;
    transaction.execute(
        "INSERT INTO source_index_meta(key, value) VALUES('schema-version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [MULTI_SOURCE_SCHEMA_VERSION.to_string()],
    )?;
    transaction.commit()
}

fn ensure_source_event_columns(connection: &Connection) -> rusqlite::Result<()> {
    let mut statement = connection.prepare("PRAGMA table_info(source_events)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    for (name, definition) in [
        (
            "cache_write_5m_tokens",
            "INTEGER NOT NULL DEFAULT 0 CHECK(cache_write_5m_tokens >= 0)",
        ),
        (
            "cache_write_1h_tokens",
            "INTEGER NOT NULL DEFAULT 0 CHECK(cache_write_1h_tokens >= 0)",
        ),
        (
            "cache_write_unknown_tokens",
            "INTEGER NOT NULL DEFAULT 0 CHECK(cache_write_unknown_tokens >= 0)",
        ),
        (
            "cache_metrics_available",
            "INTEGER NOT NULL DEFAULT 0 CHECK(cache_metrics_available IN (0, 1))",
        ),
    ] {
        if !columns.iter().any(|column| column == name) {
            connection.execute_batch(&format!(
                "ALTER TABLE source_events ADD COLUMN {name} {definition}"
            ))?;
        }
    }
    Ok(())
}

fn collect_files<F>(root: &Path, include: F) -> Vec<PathBuf>
where
    F: Fn(&Path) -> bool + Copy,
{
    let mut results = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if include(&path) {
                results.push(path);
            }
        }
    }
    results
}

fn extension_is(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}

fn is_workbuddy_trace(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(identity) = name
        .strip_prefix("trace_")
        .and_then(|name| name.strip_suffix(".json"))
    else {
        return false;
    };
    identity.len() == 32 && identity.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn file_stamp(path: &Path) -> (i64, Option<i64>) {
    let metadata = path.metadata().ok();
    let size = metadata
        .as_ref()
        .map(|value| value.len() as i64)
        .unwrap_or(0);
    let modified = metadata
        .and_then(|value| value.modified().ok())
        .and_then(system_time_ms);
    (size, modified)
}

fn database_stamp(path: &Path) -> (i64, Option<i64>) {
    let (size, modified) = file_stamp(path);
    let wal = PathBuf::from(format!("{}-wal", path.to_string_lossy()));
    let (wal_size, wal_modified) = file_stamp(&wal);
    (size + wal_size, modified.max(wal_modified))
}

fn system_time_ms(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

fn number(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64).filter(|value| *value >= 0)
}

fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn timestamp(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(value)) => value.as_i64().map(|number| {
            if number < 10_000_000_000 {
                number * 1_000
            } else {
                number
            }
        }),
        Some(Value::String(value)) => DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.timestamp_millis()),
        _ => None,
    }
}

fn local_date_from_ms(milliseconds: i64) -> Option<chrono::NaiveDate> {
    Local
        .timestamp_millis_opt(milliseconds)
        .single()
        .map(|date| date.date_naive())
}

fn recent_hour_start(milliseconds: i64, now_ms: i64) -> Option<i64> {
    const HOUR_MS: i64 = 60 * 60 * 1_000;
    let local_hour_start = |value| {
        Local
            .timestamp_millis_opt(value)
            .single()?
            .with_minute(0)?
            .with_second(0)?
            .with_nanosecond(0)
            .map(|date| date.timestamp_millis())
    };
    let current_hour = local_hour_start(now_ms)?;
    let hour_start = local_hour_start(milliseconds)?;
    (hour_start >= current_hour - 23 * HOUR_MS && hour_start <= current_hour).then_some(hour_start)
}

fn project_label(value: &str) -> String {
    value
        .trim_matches(|character| character == '/' || character == '\\')
        .rsplit(['/', '\\'])
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("未知项目")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn claude_line(session: &str, message: &str, extra: &str) -> String {
        format!(
            r#"{{"type":"assistant","sessionId":"{session}","uuid":"{message}","timestamp":"2026-08-28T00:00:00Z","cwd":"C:\\Users\\稚青\\private-project","message":{{"id":"{message}","model":"claude-test","content":"不应保存的对话正文 {extra}","usage":{{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":40}}}}}}"#,
        )
    }

    fn workbuddy_trace(total: i64) -> String {
        r#"{"sessionId":"private-session","timestamp":1787875200000,"providerData":{"requestModelId":"workbuddy-model","rawUsage":{"prompt_tokens":100,"prompt_cache_hit_tokens":20,"prompt_cache_write_tokens":10,"completion_tokens":30,"completion_thinking_tokens":7,"total_tokens":"#.to_owned()
            + &total.to_string()
            + r#"}}}"#
    }

    fn create_opencode_database(path: &Path) -> Connection {
        let connection = Connection::open(path).expect("open test OpenCode database");
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, model TEXT);
             CREATE TABLE part (id TEXT PRIMARY KEY, session_id TEXT, time_updated INTEGER, data TEXT);",
        ).expect("create OpenCode tables");
        connection
            .execute(
                "INSERT INTO session(id, directory, model) VALUES(?1, ?2, ?3)",
                params![
                    "session-private",
                    "C:\\Users\\稚青\\OpenCode Project",
                    r#"{"modelID":"open-model"}"#
                ],
            )
            .expect("insert session");
        connection.execute(
            "INSERT INTO part(id, session_id, time_updated, data) VALUES(?1, ?2, ?3, ?4)",
            params!["part-private", "session-private", 1_787_875_200_000_i64, r#"{"type":"step-finish","tokens":{"input":10,"output":20,"reasoning":4,"cache":{"read":3,"write":2},"total":35}}"#],
        ).expect("insert step finish");
        connection
    }

    #[test]
    fn claude_usage_keeps_cache_write_out_of_fresh_input() {
        let lines = "{\"type\":\"assistant\",\"sessionId\":\"s\",\"uuid\":\"m\",\"timestamp\":\"2026-08-28T00:00:00Z\",\"cwd\":\"C:\\\\Work\\\\demo\",\"message\":{\"id\":\"m\",\"model\":\"claude\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":20,\"cache_creation_input_tokens\":30,\"output_tokens\":40}}}\n";
        let events = parse_claude_file(Path::new("fixture.jsonl"), lines);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usage.total_tokens, 100);
        assert_eq!(events[0].usage.cache_write_tokens, 30);
    }

    #[test]
    fn claude_preserves_five_minute_and_one_hour_cache_writes() {
        let lines = "{\"type\":\"assistant\",\"sessionId\":\"s\",\"uuid\":\"m\",\"timestamp\":\"2026-08-28T00:00:00Z\",\"message\":{\"model\":\"claude-sonnet-4\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":20,\"cache_creation\":{\"ephemeral_5m_input_tokens\":30,\"ephemeral_1h_input_tokens\":40},\"output_tokens\":50}}}\n";
        let events = parse_claude_file(Path::new("fixture.jsonl"), lines);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usage.cache_write_5m_tokens, 30);
        assert_eq!(events[0].usage.cache_write_1h_tokens, 40);
        assert_eq!(events[0].usage.cache_write_unknown_tokens, 0);
        assert!(events[0].cache_metrics_available);
    }

    #[test]
    fn task_alert_requires_complete_cache_metrics_and_strictly_below_ninety_percent() {
        let low = task_detail(
            Some(1_787_875_200_000),
            SOURCE_CODEX.to_owned(),
            "gpt-5.6".to_owned(),
            "private-project".to_owned(),
            DeviceTokenUsage::from_parts(11, 89, 0, 1, 0, Some(101)),
            true,
        );
        assert!(low.low_cache_hit);
        assert_eq!(low.cache_hit_rate, Some(0.89));

        let threshold = task_detail(
            Some(1_787_875_200_000),
            SOURCE_CODEX.to_owned(),
            "gpt-5.6".to_owned(),
            "private-project".to_owned(),
            DeviceTokenUsage::from_parts(10, 90, 0, 1, 0, Some(101)),
            true,
        );
        assert!(!threshold.low_cache_hit);

        let unavailable = task_detail(
            Some(1_787_875_200_000),
            SOURCE_CODEX.to_owned(),
            "gpt-5.6".to_owned(),
            "private-project".to_owned(),
            DeviceTokenUsage::from_parts(1, 1, 0, 1, 0, Some(3)),
            false,
        );
        assert_eq!(unavailable.cache_hit_rate, None);
        assert!(!unavailable.low_cache_hit);
    }

    #[test]
    fn claude_skips_damaged_and_incomplete_records_without_duplicate_events() {
        let temporary = tempdir().expect("temp dir");
        let projects = temporary.path().join(".claude").join("projects");
        fs::create_dir_all(&projects).expect("create projects");
        let first = projects.join("first.jsonl");
        let second = projects.join("second.jsonl");
        fs::write(
            &first,
            format!(
                "{{not json}}\n{}",
                claude_line("private-session", "same-message", "secret")
            ),
        )
        .expect("write incomplete file");

        let mut index = MultiSourceIndex::open(&temporary.path().join("usage-index-v2.sqlite3"))
            .expect("open index");
        let first_scan = index.scan_claude(&temporary.path().join(".claude"));
        assert_eq!(first_scan.scan.expect("scan statistics").skipped_records, 1);
        assert_eq!(
            index
                .aggregate_source(SourceState::ready(SOURCE_CLAUDE, "Claude Code"))
                .indexed_events,
            0,
            "incomplete final JSONL line is deferred"
        );

        fs::write(
            &first,
            format!(
                "{{not json}}\n{}\n",
                claude_line("private-session", "same-message", "secret")
            ),
        )
        .expect("finish file");
        fs::write(
            &second,
            format!(
                "{}\n",
                claude_line("private-session", "same-message", "duplicate")
            ),
        )
        .expect("write copied event");
        let mut progress = Vec::new();
        let changed_scan = index
            .scan_claude_with_progress(&temporary.path().join(".claude"), |processed, total| {
                progress.push((processed, total))
            });
        let changed_stats = changed_scan.scan.expect("scan statistics");
        assert_eq!(changed_stats.new_events, 1);
        assert_eq!(changed_stats.skipped_records, 1);
        assert_eq!(progress.first(), Some(&(0, 2)));
        assert_eq!(progress.last(), Some(&(2, 2)));
        assert_eq!(
            index
                .aggregate_source(SourceState::ready(SOURCE_CLAUDE, "Claude Code"))
                .indexed_events,
            1,
            "session/message identity deduplicates copied JSONL events"
        );

        let unchanged_scan = index.scan_claude(&temporary.path().join(".claude"));
        assert_eq!(unchanged_scan.scan.expect("scan statistics").new_events, 0);
        assert_eq!(
            index
                .aggregate_source(SourceState::ready(SOURCE_CLAUDE, "Claude Code"))
                .indexed_events,
            1,
            "second scan is idempotent"
        );

        fs::remove_file(&first).expect("remove one copied file");
        index.scan_claude(&temporary.path().join(".claude"));
        assert_eq!(
            index
                .aggregate_source(SourceState::ready(SOURCE_CLAUDE, "Claude Code"))
                .indexed_events,
            1,
            "deleting one duplicate keeps the remaining mapping"
        );

        fs::remove_file(&second).expect("remove last copied file");
        index.scan_claude(&temporary.path().join(".claude"));
        assert_eq!(
            index
                .aggregate_source(SourceState::ready(SOURCE_CLAUDE, "Claude Code"))
                .indexed_events,
            0,
            "file deletion removes orphaned events"
        );
    }

    #[test]
    fn workbuddy_prefers_raw_usage_without_double_counting_fallbacks() {
        let trace = "{\"sessionId\":\"s\",\"timestamp\":1787875200000,\"providerData\":{\"requestModelId\":\"model\",\"rawUsage\":{\"prompt_tokens\":100,\"prompt_cache_hit_tokens\":20,\"prompt_cache_write_tokens\":10,\"completion_tokens\":30,\"total_tokens\":130},\"usage\":{\"inputTokens\":999,\"outputTokens\":999}}}";
        let events = parse_workbuddy_trace(Path::new("trace_one.json"), trace, &HashMap::new());
        assert_eq!(events[0].usage.total_tokens, 130);
        assert_eq!(events[0].usage.input_tokens, 70);
    }

    #[test]
    fn workbuddy_falls_back_to_normalized_usage_and_skips_missing_usage() {
        let normalized = r#"{"providerData":{"usage":{"inputTokens":25,"inputTokensDetails":{"cached_tokens":5},"outputTokens":11,"outputTokensDetails":{"reasoning_tokens":3},"totalTokens":36}}}"#;
        let events = parse_workbuddy_trace(
            Path::new("trace_0123456789abcdef0123456789abcdef.json"),
            normalized,
            &HashMap::new(),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usage.input_tokens, 20);
        assert_eq!(events[0].usage.reasoning_output_tokens, 3);
        assert!(parse_workbuddy_trace(
            Path::new("trace_0123456789abcdef0123456789abcdef.json"),
            "{}",
            &HashMap::new()
        )
        .is_empty());
    }

    #[test]
    fn workbuddy_reads_trace_summary_when_provider_usage_is_absent() {
        let trace = r#"{"trace":{"traceId":"private-trace","sessionId":"private-session","startedAt":"2026-08-28T00:00:00Z","totalTokens":110,"modelInfo":{"models":["workbuddy-model"],"totalInputTokens":100,"totalCachedTokens":30,"totalOutputTokens":10}}}"#;
        let events = parse_workbuddy_trace(
            Path::new("trace_0123456789abcdef0123456789abcdef.json"),
            trace,
            &HashMap::new(),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usage.input_tokens, 70);
        assert_eq!(events[0].usage.cached_input_tokens, 30);
        assert_eq!(events[0].usage.total_tokens, 110);
        assert_eq!(events[0].model_label, "workbuddy-model");
    }

    #[test]
    fn workbuddy_sources_are_separate_and_backups_are_not_scanned() {
        let temporary = tempdir().expect("temp dir");
        let workbuddy = temporary.path().join(".workbuddy");
        let workbuddy_ai = temporary.path().join(".workbuddy-ai");
        let workbuddy_traces = workbuddy.join("traces").join("1");
        let workbuddy_ai_traces = workbuddy_ai.join("traces").join("2");
        fs::create_dir_all(&workbuddy_traces).expect("create WorkBuddy traces");
        fs::create_dir_all(&workbuddy_ai_traces).expect("create WorkBuddy AI traces");
        fs::write(
            workbuddy_traces.join("trace_0123456789abcdef0123456789abcdef.json"),
            workbuddy_trace(130),
        )
        .expect("write WorkBuddy trace");
        fs::write(
            workbuddy_traces.join("trace_0123456789abcdef0123456789abcdef.backup.json"),
            workbuddy_trace(9_999),
        )
        .expect("write ignored backup");
        fs::write(
            workbuddy_ai_traces.join("trace_fedcba9876543210fedcba9876543210.json"),
            workbuddy_trace(230),
        )
        .expect("write WorkBuddy AI trace");

        let mut index = MultiSourceIndex::open(&temporary.path().join("usage-index-v2.sqlite3"))
            .expect("open index");
        index.scan_workbuddy(SOURCE_WORKBUDDY, "WorkBuddy", &workbuddy);
        index.scan_workbuddy(SOURCE_WORKBUDDY_AI, "WorkBuddy AI", &workbuddy_ai);
        let standard = index.aggregate_source(SourceState::ready(SOURCE_WORKBUDDY, "WorkBuddy"));
        let ai = index.aggregate_source(SourceState::ready(SOURCE_WORKBUDDY_AI, "WorkBuddy AI"));
        assert_eq!(standard.indexed_events, 1);
        assert_eq!(standard.total.total_tokens, 130);
        assert_eq!(ai.indexed_events, 1);
        assert_eq!(ai.total.total_tokens, 230);
    }

    #[test]
    fn opencode_reads_active_wal_records_and_incrementally_replaces_parts() {
        let temporary = tempdir().expect("temp dir");
        let database = temporary.path().join("opencode.db");
        let writer = create_opencode_database(&database);
        let wal = PathBuf::from(format!("{}-wal", database.to_string_lossy()));
        assert!(
            wal.is_file(),
            "the test record remains available through SQLite WAL"
        );
        let events = read_opencode_events(&database).expect("read OpenCode WAL");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].usage.cache_write_tokens, 2);
        assert_eq!(events[0].usage.reasoning_output_tokens, 4);

        let mut index = MultiSourceIndex::open(&temporary.path().join("usage-index-v2.sqlite3"))
            .expect("open index");
        index.scan_opencode(&database);
        index.scan_opencode(&database);
        assert_eq!(
            index
                .aggregate_source(SourceState::ready(SOURCE_OPENCODE, "OpenCode"))
                .indexed_events,
            1
        );

        writer.execute(
            "UPDATE part SET time_updated = ?1, data = ?2 WHERE id = ?3",
            params![1_787_875_200_001_i64, r#"{"type":"step-finish","tokens":{"input":20,"output":20,"cache":{"read":3,"write":2},"total":45}}"#, "part-private"],
        ).expect("update active WAL part");
        index.scan_opencode(&database);
        let source = index.aggregate_source(SourceState::ready(SOURCE_OPENCODE, "OpenCode"));
        assert_eq!(source.indexed_events, 1);
        assert_eq!(source.total.total_tokens, 45);
    }

    #[test]
    fn v2_index_and_serialized_summary_do_not_retain_private_source_data() {
        let temporary = tempdir().expect("temp dir");
        let projects = temporary.path().join(".claude").join("projects");
        fs::create_dir_all(&projects).expect("create projects");
        fs::write(
            projects.join("private.jsonl"),
            format!(
                "{}\n",
                claude_line(
                    "very-private-session",
                    "very-private-message",
                    "TOP_SECRET_PROMPT_TEXT"
                )
            ),
        )
        .expect("write fixture");
        let database = temporary.path().join("usage-index-v2.sqlite3");
        let mut index = MultiSourceIndex::open(&database).expect("open index");
        index.scan_claude(&temporary.path().join(".claude"));
        let summary = index.aggregate_source(SourceState::ready(SOURCE_CLAUDE, "Claude Code"));
        let serialized = serde_json::to_string(&summary).expect("serialize summary");
        for private_value in [
            "TOP_SECRET_PROMPT_TEXT",
            "very-private-session",
            "very-private-message",
            "C:\\Users\\稚青",
        ] {
            assert!(!serialized.contains(private_value));
        }
        drop(index);
        let database_bytes = fs::read(&database).expect("read index database");
        let wal = PathBuf::from(format!("{}-wal", database.to_string_lossy()));
        let wal_bytes = fs::read(&wal).unwrap_or_default();
        let indexed_text = format!(
            "{}{}",
            String::from_utf8_lossy(&database_bytes),
            String::from_utf8_lossy(&wal_bytes)
        );
        for private_value in [
            "TOP_SECRET_PROMPT_TEXT",
            "very-private-session",
            "very-private-message",
            "C:\\Users\\稚青",
        ] {
            assert!(!indexed_text.contains(private_value));
        }
    }

    #[test]
    fn cursor_remains_unavailable_with_no_usage() {
        let source = empty_source_summary(SourceState {
            id: SOURCE_CURSOR,
            label: "Cursor",
            status: "unavailable",
            message: Some("等待官方数据来源。".to_owned()),
            credits: None,
            scan: None,
            last_indexed_at: None,
        });
        assert_eq!(source.status, "unavailable");
        assert_eq!(source.total.total_tokens, 0);
        assert_eq!(source.indexed_events, 0);
    }

    #[test]
    fn today_cost_and_models_exclude_events_without_a_reliable_timestamp() {
        let temporary = tempdir().expect("temp dir");
        let mut index = MultiSourceIndex::open(&temporary.path().join("usage-index-v2.sqlite3"))
            .expect("open index");
        let file_key = hash_bytes(b"fixture-source-file");
        let current_usage = DeviceTokenUsage::from_parts(1_000, 0, 0, 0, 0, Some(1_000));
        let unpriced_usage = DeviceTokenUsage::from_parts(500, 0, 0, 0, 0, Some(500));
        {
            let transaction = index.connection.transaction().expect("transaction");
            insert_event(
                &transaction,
                SOURCE_CLAUDE,
                &file_key,
                UsageEvent {
                    identity: "dated".to_owned(),
                    occurred_at_ms: Some(Local::now().timestamp_millis()),
                    model_label: "claude-sonnet-4-20250514".to_owned(),
                    project_label: "fixture".to_owned(),
                    usage: current_usage,
                    cache_metrics_available: true,
                },
            )
            .expect("dated event");
            insert_event(
                &transaction,
                SOURCE_CLAUDE,
                &file_key,
                UsageEvent {
                    identity: "undated".to_owned(),
                    occurred_at_ms: None,
                    model_label: "unknown-model".to_owned(),
                    project_label: "fixture".to_owned(),
                    usage: unpriced_usage,
                    cache_metrics_available: false,
                },
            )
            .expect("undated event");
            transaction.commit().expect("commit");
        }

        let summary = index.aggregate_source(SourceState::ready(SOURCE_CLAUDE, "Claude Code"));
        assert_eq!(summary.total.total_tokens, 1_500);
        assert_eq!(summary.today.total_tokens, 1_000);
        assert_eq!(summary.today_by_model.len(), 1);
        assert_eq!(summary.today_by_model[0].usage.total_tokens, 1_000);
        assert_eq!(summary.today_cost.unpriced_tokens, 0);
        assert!(summary.cost.unpriced_tokens >= 500);
        assert_eq!(summary.ranges.today.usage.total_tokens, 1_000);
        assert_eq!(summary.ranges.last7_days.usage.total_tokens, 1_000);
        assert_eq!(summary.ranges.last30_days.daily_usage.len(), 1);
        assert_eq!(summary.ranges.total.usage.total_tokens, 1_500);
        let today_model_cost =
            summary
                .today_by_model
                .iter()
                .fold(CostSummary::default(), |mut total, model| {
                    total.add_assign(&model.cost);
                    total
                });
        assert_eq!(today_model_cost, summary.today_cost);
    }

    #[test]
    fn workbuddy_credit_balance_is_derived_without_entering_token_usage() {
        let temporary = tempdir().expect("temp dir");
        let database = temporary.path().join("workbuddy.db");
        let connection = Connection::open(&database).expect("open database");
        connection
            .execute_batch(
                "CREATE TABLE session_usage (
                   session_id TEXT PRIMARY KEY,
                   used INTEGER NOT NULL,
                   size INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL,
                   credit_json TEXT
                 );",
            )
            .expect("create session usage table");
        connection
            .execute(
                "INSERT INTO session_usage VALUES(?1, ?2, ?3, ?4, ?5)",
                params![
                    "private-session",
                    1_i64,
                    1_i64,
                    2_i64,
                    r#"{"balance":12.5,"secret":"never persist this"}"#
                ],
            )
            .expect("insert credit value");
        let credits = read_workbuddy_credit_balance(&database).expect("derive credit balance");
        assert_eq!(credits.balance, 12.5);
    }

    #[test]
    fn project_labels_never_keep_a_full_path() {
        assert_eq!(project_label("C:\\Users\\稚青\\demo"), "demo");
    }

    #[test]
    fn rolling_hour_window_includes_previous_day_and_excludes_older_events() {
        const HOUR_MS: i64 = 60 * 60 * 1_000;
        let now_ms = Utc::now().timestamp_millis();
        let current_hour = recent_hour_start(now_ms, now_ms).expect("current local hour");
        assert_eq!(
            recent_hour_start(current_hour - 23 * HOUR_MS, now_ms),
            Some(current_hour - 23 * HOUR_MS)
        );
        assert_eq!(recent_hour_start(current_hour - 24 * HOUR_MS, now_ms), None);
        assert_eq!(recent_hour_start(current_hour + HOUR_MS, now_ms), None);
    }
}
