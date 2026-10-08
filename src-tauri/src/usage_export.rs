//! Reports built only from existing, numeric indexes. No source-log collection.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::{Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    index::IncrementalIndex,
    multisource::{self, DeviceTokenUsage, IndexedExportEvent},
    pricing::{CostSummary, PricingSettings},
    settings::{self, ProjectMergeRule},
};

const SOURCES: &[(&str, &str)] = &[
    ("codex", "Codex"),
    ("claude-code", "Claude Code"),
    ("opencode", "OpenCode"),
    ("workbuddy", "WorkBuddy"),
    ("workbuddy-ai", "WorkBuddy AI"),
    ("cursor", "Cursor"),
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum ExportRange {
    All,
    Dates {
        #[serde(rename = "startDate")]
        start_date: String,
        #[serde(rename = "endDate")]
        end_date: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UsageExportQuery {
    pub(crate) range: ExportRange,
    pub(crate) source_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportMetrics {
    pub(crate) usage: DeviceTokenUsage,
    pub(crate) cost: CostSummary,
}

impl ExportMetrics {
    fn add(&mut self, usage: DeviceTokenUsage, cost: &CostSummary) {
        self.usage.add_assign(usage);
        self.cost.add_assign(cost);
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelRow {
    source_id: String,
    source_label: String,
    model: String,
    #[serde(flatten)]
    metrics: ExportMetrics,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DailyRow {
    date: Option<String>,
    source_id: String,
    source_label: String,
    #[serde(flatten)]
    metrics: ExportMetrics,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DailyModelRow {
    date: Option<String>,
    source_id: String,
    source_label: String,
    model: String,
    #[serde(flatten)]
    metrics: ExportMetrics,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceRow {
    source_id: String,
    source_label: String,
    status: String,
    index_read_status: &'static str,
    last_indexed_at: Option<String>,
    collection_status_observed_at: Option<String>,
    #[serde(flatten)]
    metrics: ExportMetrics,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectRow {
    project: String,
    source_ids: Vec<String>,
    source_labels: Vec<String>,
    merged: bool,
    #[serde(flatten)]
    metrics: ExportMetrics,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageExportReport {
    schema_version: u8,
    generated_at: String,
    query: UsageExportQuery,
    time_zone: &'static str,
    utc_offset_minutes: i32,
    last_collection_at: Option<String>,
    pricing_updated_at: Option<String>,
    summary: ExportMetrics,
    models: Vec<ModelRow>,
    daily: Vec<DailyRow>,
    daily_models: Vec<DailyModelRow>,
    sources: Vec<SourceRow>,
    projects: Vec<ProjectRow>,
}

fn source_label(id: &str) -> String {
    SOURCES
        .iter()
        .find(|(source, _)| *source == id)
        .map(|(_, label)| (*label).to_owned())
        .unwrap_or_else(|| id.to_owned())
}

fn parse_date(value: &str) -> Result<NaiveDate, String> {
    if value.len() != 10
        || !value.bytes().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
    {
        return Err("日期格式应为 YYYY-MM-DD。".to_owned());
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| "请输入有效的起止日期。".to_owned())?;
    if date.format("%Y-%m-%d").to_string() != value {
        return Err("日期格式应为 YYYY-MM-DD。".to_owned());
    }
    Ok(date)
}

fn range_bounds(range: &ExportRange) -> Result<Option<(i64, i64)>, String> {
    range_bounds_in_zone(range, &Local)
}

fn range_bounds_in_zone<T: TimeZone>(
    range: &ExportRange,
    zone: &T,
) -> Result<Option<(i64, i64)>, String> {
    match range {
        ExportRange::All => Ok(None),
        ExportRange::Dates {
            start_date,
            end_date,
        } => {
            let start = parse_date(start_date)?;
            let end = parse_date(end_date)?;
            if start > end {
                return Err("开始日期不能晚于结束日期。".to_owned());
            }
            let next = end
                .succ_opt()
                .ok_or_else(|| "结束日期超出支持范围。".to_owned())?;
            let midnight = |day: NaiveDate| {
                zone.from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
                    .earliest()
                    .map(|time| time.timestamp_millis())
                    .ok_or_else(|| "该日期的本机零点不存在，请调整日期范围。".to_owned())
            };
            Ok(Some((midnight(start)?, midnight(next)?)))
        }
    }
}

fn validate_query(query: &UsageExportQuery) -> Result<(), String> {
    if query.source_ids.is_empty() {
        return Err("请至少选择一个数据来源。".to_owned());
    }
    let mut seen = BTreeSet::new();
    for id in &query.source_ids {
        if !SOURCES.iter().any(|(source, _)| *source == id.as_str()) || !seen.insert(id) {
            return Err("数据来源无效或重复。".to_owned());
        }
    }
    if query.source_ids.iter().all(|id| id == "cursor") {
        return Err("Cursor 暂无可导出的本机精确 Token 数据。".to_owned());
    }
    range_bounds(&query.range)?;
    Ok(())
}

/// The caller must hold UsageRuntime::refresh_gate until this function returns.
pub(crate) fn read_report(
    query: UsageExportQuery,
    codex_path: &Path,
    multi_path: &Path,
    settings_path: &Path,
    last_snapshot: Option<&Value>,
) -> Result<UsageExportReport, String> {
    validate_query(&query)?;
    let bounds = range_bounds(&query.range)?;
    let (pricing, rules) = settings::read_export_settings(settings_path)
        .map_err(|_| "无法读取已保存的定价或项目合并设置，请检查设置后重试。".to_owned())?;
    let mut events = Vec::new();
    if query.source_ids.iter().any(|id| id == "codex") {
        if !codex_path.is_file() {
            return Err("Codex 索引尚未建立，请先刷新本机用量。".to_owned());
        }
        let codex = IncrementalIndex::read_existing_events(codex_path, bounds)
            .map_err(|_| "无法读取现有 Codex 索引，请先刷新或检查索引。".to_owned())?;
        events.extend(codex.into_iter().map(|event| IndexedExportEvent {
            source_id: "codex".to_owned(),
            occurred_at_ms: event.occurred_at_ms,
            model_label: event.model_label,
            project_label: event.project_label,
            usage: DeviceTokenUsage::from_parts(
                event.usage.input_tokens - event.usage.cached_input_tokens,
                event.usage.cached_input_tokens,
                0,
                event.usage.output_tokens,
                event.usage.reasoning_output_tokens,
                Some(event.usage.total_tokens),
            ),
        }));
    }
    let other_sources: Vec<_> = query
        .source_ids
        .iter()
        .filter(|id| id.as_str() != "codex" && id.as_str() != "cursor")
        .cloned()
        .collect();
    if !other_sources.is_empty() {
        if !multi_path.is_file() {
            return Err("其他来源的索引尚未建立，请先刷新本机用量。".to_owned());
        }
        events.extend(
            multisource::read_existing_export_events(multi_path, &other_sources, bounds)
                .map_err(|_| "无法完整读取其他来源索引，请先刷新或检查索引。".to_owned())?,
        );
    }
    build_report(query, events, &pricing, &rules, last_snapshot)
}

fn build_report(
    query: UsageExportQuery,
    events: Vec<IndexedExportEvent>,
    pricing: &PricingSettings,
    rules: &[ProjectMergeRule],
    snapshot: Option<&Value>,
) -> Result<UsageExportReport, String> {
    let now = Local::now();
    let last_collection_at = snapshot
        .and_then(|value| value.pointer("/deviceUsage/generatedAt"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut summary = ExportMetrics::default();
    let mut model_totals = BTreeMap::<(String, String), ExportMetrics>::new();
    let mut daily_totals = BTreeMap::<(Option<String>, String), ExportMetrics>::new();
    let mut daily_models = BTreeMap::<(Option<String>, String, String), ExportMetrics>::new();
    let mut source_totals = BTreeMap::<String, ExportMetrics>::new();
    let mut projects =
        BTreeMap::<(bool, String, String), (String, BTreeSet<String>, ExportMetrics)>::new();
    let mut membership = BTreeMap::new();
    for rule in rules {
        for member in &rule.members {
            membership.insert((member.source_id.clone(), member.project_key.clone()), rule);
            membership.insert(
                (member.source_id.clone(), member.project_label.clone()),
                rule,
            );
        }
    }
    for event in events {
        let date = event
            .occurred_at_ms
            .and_then(|value| Local.timestamp_millis_opt(value).single())
            .map(|value| value.format("%Y-%m-%d").to_string());
        let cost = multisource::cost_for(
            &event.model_label,
            event.usage,
            event.occurred_at_ms,
            pricing,
        );
        summary.add(event.usage, &cost);
        model_totals
            .entry((event.source_id.clone(), event.model_label.clone()))
            .or_default()
            .add(event.usage, &cost);
        daily_totals
            .entry((date.clone(), event.source_id.clone()))
            .or_default()
            .add(event.usage, &cost);
        daily_models
            .entry((date, event.source_id.clone(), event.model_label))
            .or_default()
            .add(event.usage, &cost);
        source_totals
            .entry(event.source_id.clone())
            .or_default()
            .add(event.usage, &cost);
        let rule = membership.get(&(event.source_id.clone(), event.project_label.clone()));
        let (key, label) = if let Some(rule) = rule {
            (
                (true, rule.id.clone(), String::new()),
                rule.display_name.clone(),
            )
        } else {
            (
                (false, event.source_id.clone(), event.project_label.clone()),
                event.project_label,
            )
        };
        let project = projects
            .entry(key)
            .or_insert_with(|| (label, BTreeSet::new(), ExportMetrics::default()));
        project.1.insert(event.source_id);
        project.2.add(event.usage, &cost);
    }
    let span = match &query.range {
        ExportRange::Dates {
            start_date,
            end_date,
        } => Some((parse_date(start_date)?, parse_date(end_date)?)),
        ExportRange::All => {
            let dates: Vec<_> = daily_totals
                .keys()
                .filter_map(|(date, _)| date.as_deref())
                .collect();
            match (dates.iter().min(), dates.iter().max()) {
                (Some(start), Some(end)) => Some((parse_date(start)?, parse_date(end)?)),
                _ => None,
            }
        }
    };
    if let Some((mut day, end)) = span {
        loop {
            let date = day.format("%Y-%m-%d").to_string();
            for id in query.source_ids.iter().filter(|id| id.as_str() != "cursor") {
                daily_totals
                    .entry((Some(date.clone()), id.clone()))
                    .or_default();
            }
            if day == end {
                break;
            }
            day = day
                .succ_opt()
                .ok_or_else(|| "日期范围超出支持范围。".to_owned())?;
        }
    }
    let mut models: Vec<_> = model_totals
        .into_iter()
        .map(|((source_id, model), metrics)| ModelRow {
            source_label: source_label(&source_id),
            source_id,
            model,
            metrics,
        })
        .collect();
    models.sort_by(|a, b| {
        b.metrics
            .usage
            .total_tokens
            .cmp(&a.metrics.usage.total_tokens)
            .then_with(|| (&a.model, &a.source_id).cmp(&(&b.model, &b.source_id)))
    });
    let mut daily: Vec<_> = daily_totals
        .into_iter()
        .map(|((date, source_id), metrics)| DailyRow {
            date,
            source_label: source_label(&source_id),
            source_id,
            metrics,
        })
        .collect();
    daily.sort_by(|a, b| {
        (a.date.is_none(), &a.date, &a.source_id).cmp(&(b.date.is_none(), &b.date, &b.source_id))
    });
    let mut daily_models: Vec<_> = daily_models
        .into_iter()
        .map(|((date, source_id, model), metrics)| DailyModelRow {
            date,
            source_label: source_label(&source_id),
            source_id,
            model,
            metrics,
        })
        .collect();
    daily_models.sort_by(|a, b| {
        (a.date.is_none(), &a.date, &a.source_id, &a.model).cmp(&(
            b.date.is_none(),
            &b.date,
            &b.source_id,
            &b.model,
        ))
    });
    let sources = query
        .source_ids
        .iter()
        .map(|id| {
            let health = snapshot
                .and_then(|value| value.get("sourceHealth"))
                .and_then(Value::as_array)
                .and_then(|rows| {
                    rows.iter()
                        .find(|row| row.get("id").and_then(Value::as_str) == Some(id.as_str()))
                });
            let status = health
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str)
                .filter(|value| ["ready", "notDetected", "unavailable", "error"].contains(value))
                .unwrap_or(if id == "cursor" {
                    "unavailable"
                } else {
                    "unknown"
                });
            SourceRow {
                source_id: id.clone(),
                source_label: source_label(id),
                status: status.to_owned(),
                index_read_status: if id == "cursor" {
                    "notApplicable"
                } else {
                    "readable"
                },
                last_indexed_at: health
                    .and_then(|value| value.get("lastIndexedAt"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                collection_status_observed_at: health.and(last_collection_at.clone()),
                metrics: source_totals.remove(id).unwrap_or_default(),
            }
        })
        .collect();
    let mut projects: Vec<_> = projects
        .into_iter()
        .map(|((merged, _, _), (project, sources, metrics))| {
            let source_ids: Vec<_> = sources.into_iter().collect();
            ProjectRow {
                source_labels: source_ids.iter().map(|id| source_label(id)).collect(),
                source_ids,
                project,
                merged,
                metrics,
            }
        })
        .collect();
    projects.sort_by(|a, b| {
        b.metrics
            .usage
            .total_tokens
            .cmp(&a.metrics.usage.total_tokens)
            .then_with(|| (&a.project, &a.source_ids).cmp(&(&b.project, &b.source_ids)))
    });
    Ok(UsageExportReport {
        schema_version: 1,
        generated_at: Utc::now().to_rfc3339(),
        query,
        time_zone: "system-local",
        utc_offset_minutes: now.offset().local_minus_utc() / 60,
        last_collection_at,
        pricing_updated_at: pricing.updated_at.clone(),
        summary,
        models,
        daily,
        daily_models,
        sources,
        projects,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ExportFormat {
    Csv,
    Json,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveExportRequest {
    format: ExportFormat,
    suggested_file_name: String,
    content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveExportResult {
    status: &'static str,
    path: Option<String>,
}

pub(crate) fn save_report(
    request: SaveExportRequest,
    window: tauri::Window,
) -> Result<SaveExportResult, String> {
    let extension = match request.format {
        ExportFormat::Csv => "csv",
        ExportFormat::Json => "json",
    };
    if request.suggested_file_name.is_empty()
        || request.suggested_file_name.contains(['/', '\\', ':'])
    {
        return Err("导出文件名无效。".to_owned());
    }
    if let ExportFormat::Json = request.format {
        serde_json::from_str::<Value>(&request.content)
            .map_err(|_| "JSON 报告格式无效。".to_owned())?;
    }
    let Some(path) = rfd::FileDialog::new()
        .set_parent(&window)
        .set_title("保存用量报告")
        .add_filter(extension.to_uppercase(), &[extension])
        .set_file_name(&request.suggested_file_name)
        .save_file()
    else {
        return Ok(SaveExportResult {
            status: "cancelled",
            path: None,
        });
    };
    if path
        .extension()
        .and_then(|value| value.to_str())
        .map_or(true, |value| !value.eq_ignore_ascii_case(extension))
    {
        return Err(format!("请使用 .{extension} 文件扩展名。"));
    }
    atomic_write(&path, request.content.as_bytes()).map_err(|_| {
        "无法保存报告，请检查目标文件是否被占用，以及保存位置的写入权限。".to_owned()
    })?;
    Ok(SaveExportResult {
        status: "saved",
        path: Some(path.to_string_lossy().into_owned()),
    })
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing directory")
    })?;
    let temp = parent.join(format!(
        ".ai-token-export-{}-{}-{}.tmp",
        std::process::id(),
        Utc::now().timestamp_millis(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn query(range: ExportRange) -> UsageExportQuery {
        UsageExportQuery {
            range,
            source_ids: vec![
                "codex".to_owned(),
                "claude-code".to_owned(),
                "cursor".to_owned(),
            ],
        }
    }

    fn event(
        source: &str,
        model: &str,
        project: &str,
        date: Option<&str>,
        tokens: i64,
    ) -> IndexedExportEvent {
        IndexedExportEvent {
            source_id: source.to_owned(),
            model_label: model.to_owned(),
            project_label: project.to_owned(),
            occurred_at_ms: date.map(|date| {
                Local
                    .from_local_datetime(&parse_date(date).unwrap().and_hms_opt(12, 0, 0).unwrap())
                    .earliest()
                    .unwrap()
                    .timestamp_millis()
            }),
            usage: DeviceTokenUsage::from_parts(tokens, 0, 0, 0, 0, Some(tokens)),
        }
    }

    #[test]
    fn all_dimensions_reconcile_with_undated_events_and_project_merges() {
        let rules = vec![ProjectMergeRule {
            id: "merge-one".into(),
            display_name: "合并项目".into(),
            members: vec![
                settings::ProjectMergeMember {
                    source_id: "codex".into(),
                    source_label: "Codex".into(),
                    project_key: "alpha".into(),
                    project_label: "alpha".into(),
                },
                settings::ProjectMergeMember {
                    source_id: "claude-code".into(),
                    source_label: "Claude Code".into(),
                    project_key: "beta".into(),
                    project_label: "beta".into(),
                },
            ],
        }];
        let report = build_report(
            query(ExportRange::All),
            vec![
                event("codex", "vendor/model", "alpha", Some("2026-01-01"), 10),
                event(
                    "claude-code",
                    "vendor/model",
                    "beta",
                    Some("2026-01-03"),
                    20,
                ),
                event("codex", "vendor/model", "alpha", None, 7),
            ],
            &PricingSettings::default(),
            &rules,
            None,
        )
        .unwrap();
        let total = report.summary.usage.total_tokens;
        assert_eq!(total, 37);
        assert_eq!(report.summary.cost.unpriced_tokens, 37);
        assert_eq!(
            report
                .models
                .iter()
                .map(|row| row.metrics.usage.total_tokens)
                .sum::<i64>(),
            total
        );
        assert_eq!(
            report
                .daily
                .iter()
                .map(|row| row.metrics.usage.total_tokens)
                .sum::<i64>(),
            total
        );
        assert_eq!(
            report
                .daily_models
                .iter()
                .map(|row| row.metrics.usage.total_tokens)
                .sum::<i64>(),
            total
        );
        assert_eq!(
            report
                .sources
                .iter()
                .map(|row| row.metrics.usage.total_tokens)
                .sum::<i64>(),
            total
        );
        assert_eq!(
            report
                .projects
                .iter()
                .map(|row| row.metrics.usage.total_tokens)
                .sum::<i64>(),
            total
        );
        assert_eq!(
            report.models.len(),
            2,
            "same model in different applications stays distinct"
        );
        assert_eq!(
            report.daily.len(),
            7,
            "three days times two readable sources plus undated"
        );
        assert_eq!(
            report.daily_models.len(),
            3,
            "no zero-filled model Cartesian product"
        );
        assert!(report.daily.last().unwrap().date.is_none());
        assert_eq!(report.projects.len(), 1);
        assert!(report.projects[0].merged);
        assert_eq!(
            report.sources.last().unwrap().index_read_status,
            "notApplicable"
        );
    }

    #[test]
    fn custom_range_over_31_days_includes_empty_days_and_end_date() {
        let range = ExportRange::Dates {
            start_date: "2026-01-01".into(),
            end_date: "2026-03-02".into(),
        };
        let (start, end) = range_bounds(&range).unwrap().unwrap();
        assert_eq!(
            Local
                .timestamp_millis_opt(start)
                .unwrap()
                .date_naive()
                .to_string(),
            "2026-01-01"
        );
        assert_eq!(
            Local
                .timestamp_millis_opt(end)
                .unwrap()
                .date_naive()
                .to_string(),
            "2026-03-03"
        );
        let report =
            build_report(query(range), vec![], &PricingSettings::default(), &[], None).unwrap();
        assert_eq!(report.daily.len(), 61 * 2);
        assert!(report.daily_models.is_empty());
        assert_eq!(report.summary.usage.total_tokens, 0);
    }

    #[test]
    fn date_boundaries_follow_local_days_across_daylight_saving_changes() {
        let zone = chrono_tz::America::New_York;
        for (date, expected_hours) in [("2026-03-08", 23), ("2026-11-01", 25)] {
            let range = ExportRange::Dates {
                start_date: date.into(),
                end_date: date.into(),
            };
            let (start, end) = range_bounds_in_zone(&range, &zone).unwrap().unwrap();
            assert_eq!((end - start) / 3_600_000, expected_hours);
        }
    }

    #[test]
    fn report_uses_event_peak_prices_keeps_currencies_and_provider_labels_separate() {
        let mut pricing = PricingSettings::default();
        pricing.peak.time_zone = "UTC".into();
        pricing.peak.multiplier = 2.0;
        pricing.peak.windows = vec![crate::pricing::PeakPricingWindow {
            weekday_start: 1,
            weekday_end: 7,
            start_time: "18:00".into(),
            end_time: "23:00".into(),
        }];
        for (model, currency, rate) in [
            ("provider-a/model", "USD", 2.0),
            ("provider-b/model", "CNY", 3.0),
        ] {
            pricing.models.push(crate::pricing::ModelPricingRule {
                model_id: model.into(),
                display_name: model.into(),
                currency: currency.into(),
                input_per_million: Some(rate),
                cached_input_per_million: None,
                cache_write_per_million: None,
                output_per_million: None,
                peak_enabled: true,
            });
        }
        let mut before = event("codex", "provider-a/model", "project", None, 1_000_000);
        before.occurred_at_ms = Some(
            chrono::DateTime::parse_from_rfc3339("2026-01-05T17:00:00Z")
                .unwrap()
                .timestamp_millis(),
        );
        let mut peak = event("codex", "provider-a/model", "project", None, 1_000_000);
        peak.occurred_at_ms = Some(
            chrono::DateTime::parse_from_rfc3339("2026-01-05T19:00:00Z")
                .unwrap()
                .timestamp_millis(),
        );
        let mut cny = event("codex", "provider-b/model", "project", None, 1_000_000);
        cny.occurred_at_ms = before.occurred_at_ms;
        let report = build_report(
            query(ExportRange::All),
            vec![before, peak, cny],
            &pricing,
            &[],
            None,
        )
        .unwrap();
        assert_eq!(report.models.len(), 2);
        assert_eq!(report.summary.cost.usd, 6.0);
        assert_eq!(report.summary.cost.cny, 3.0);
        assert_eq!(report.summary.usage.total_tokens, 3_000_000);
        assert_eq!(
            report
                .daily
                .iter()
                .map(|row| row.metrics.cost.usd)
                .sum::<f64>(),
            6.0
        );
        assert_eq!(
            report
                .daily_models
                .iter()
                .map(|row| row.metrics.cost.cny)
                .sum::<f64>(),
            3.0
        );
    }

    #[test]
    fn rejects_invalid_ranges_sources_and_unsupported_only_selection() {
        assert!(range_bounds(&ExportRange::Dates {
            start_date: "2026-02-30".into(),
            end_date: "2026-03-01".into()
        })
        .is_err());
        assert!(range_bounds(&ExportRange::Dates {
            start_date: "2026-03-01".into(),
            end_date: "2026-02-01".into()
        })
        .is_err());
        for ids in [
            vec![],
            vec!["cursor"],
            vec!["codex", "codex"],
            vec!["other"],
        ] {
            assert!(validate_query(&UsageExportQuery {
                range: ExportRange::All,
                source_ids: ids.into_iter().map(str::to_owned).collect()
            })
            .is_err());
        }
    }

    #[test]
    fn source_health_remains_last_known_separate_from_readable_index() {
        let snapshot = json!({"deviceUsage":{"generatedAt":"2026-01-01T00:00:00Z"},"sourceHealth":[{"id":"codex","status":"error","lastIndexedAt":"2025-12-31T00:00:00Z"}]});
        let report = build_report(
            query(ExportRange::All),
            vec![event("codex", "m", "p", None, 3)],
            &PricingSettings::default(),
            &[],
            Some(&snapshot),
        )
        .unwrap();
        assert_eq!(report.sources[0].status, "error");
        assert_eq!(report.sources[0].index_read_status, "readable");
        assert_eq!(
            report.sources[0].collection_status_observed_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(report.sources[0].metrics.usage.total_tokens, 3);
        let encoded = serde_json::to_string(&report).unwrap();
        assert!(!encoded.contains("projectKey"));
        assert!(!encoded.contains("revision"));
    }

    #[test]
    fn missing_indices_and_settings_are_never_created() {
        let temp = tempdir().unwrap();
        let codex = temp.path().join("missing-codex.sqlite3");
        let multi = temp.path().join("missing-multi.sqlite3");
        let settings = temp.path().join("missing-settings.sqlite3");
        assert!(read_report(query(ExportRange::All), &codex, &multi, &settings, None).is_err());
        assert!(fs::read_dir(temp.path()).unwrap().next().is_none());
    }

    #[test]
    fn native_atomic_writer_replaces_successfully_and_preserves_destination_on_failure() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("报告.json");
        fs::write(&file, b"old").unwrap();
        atomic_write(&file, b"new").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"new");
        let directory = temp.path().join("occupied.csv");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("keep"), b"untouched").unwrap();
        assert!(atomic_write(&directory, b"cannot replace directory").is_err());
        assert_eq!(fs::read(directory.join("keep")).unwrap(), b"untouched");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }
}
