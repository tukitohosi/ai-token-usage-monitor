use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_notification::NotificationExt;

use crate::{
    index::{
        IncrementalIndex, LocalUsageEvent, LocalUsageSummary, APPLICATION_ID,
        PARSER_SEMANTICS_VERSION, SCHEMA_VERSION,
    },
    multisource::{
        build_day_detail, codex_day_tasks, codex_day_tasks_with_pricing, DeviceUsageSummary,
        MultiSourceIndex, SourceScanProgress, UsageDayDetail, MULTI_SOURCE_SCHEMA_VERSION,
    },
    pricing::PricingSettings,
    settings,
    snapshot::{self, DashboardSnapshot},
};

pub(crate) const SNAPSHOT_EVENT: &str = "usage://snapshot";
pub(crate) const PROGRESS_EVENT: &str = "usage://refresh-progress";
pub(crate) const REFRESH_SCHEDULE_EVENT: &str = "usage://refresh-schedule";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IndexDatabaseHealth {
    label: &'static str,
    status: &'static str,
    integrity: &'static str,
    size_bytes: u64,
    schema_version: Option<i64>,
    expected_schema_version: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IndexMaintenanceReport {
    checked_at: String,
    overall_status: &'static str,
    databases: Vec<IndexDatabaseHealth>,
    last_backup_at: Option<String>,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IndexRebuildResult {
    pub(crate) backup_created_at: Option<String>,
    pub(crate) report: IndexMaintenanceReport,
    pub(crate) snapshot: DashboardSnapshot,
}

#[derive(Clone)]
pub(crate) struct UsageRuntime {
    app_data_directory: PathBuf,
    codex_database_path: PathBuf,
    multi_source_database_path: PathBuf,
    settings_path: PathBuf,
    snapshot_cache_path: PathBuf,
    codex_home: PathBuf,
    user_home: PathBuf,
    refresh_gate: Arc<Mutex<()>>,
    shutdown: Arc<AtomicBool>,
    event_refresh_in_flight: Arc<AtomicBool>,
    refresh_interval_seconds: Arc<AtomicU64>,
    next_refresh_at_ms: Arc<AtomicU64>,
    hide_on_close: Arc<AtomicBool>,
    active_notification_keys: Arc<Mutex<BTreeSet<String>>>,
}

impl UsageRuntime {
    pub(crate) fn new(
        app_data_directory: PathBuf,
        codex_home: PathBuf,
        user_home: PathBuf,
    ) -> Self {
        let settings_path = app_data_directory.join("settings-v1.sqlite3");
        let preferences =
            settings::read_app_preferences(&settings_path, &app_data_directory, false).ok();
        let refresh_interval_seconds = preferences
            .as_ref()
            .map(|value| value.refresh_interval_minutes * 60)
            .unwrap_or(60);
        Self {
            app_data_directory: app_data_directory.clone(),
            codex_database_path: app_data_directory.join("usage-index-v1.sqlite3"),
            multi_source_database_path: app_data_directory.join("usage-index-v2.sqlite3"),
            settings_path,
            snapshot_cache_path: app_data_directory.join("dashboard-snapshot-v1.json"),
            codex_home,
            user_home,
            refresh_gate: Arc::new(Mutex::new(())),
            shutdown: Arc::new(AtomicBool::new(false)),
            event_refresh_in_flight: Arc::new(AtomicBool::new(false)),
            refresh_interval_seconds: Arc::new(AtomicU64::new(refresh_interval_seconds)),
            next_refresh_at_ms: Arc::new(AtomicU64::new(
                epoch_milliseconds().saturating_add(refresh_interval_seconds * 1_000),
            )),
            hide_on_close: Arc::new(AtomicBool::new(
                preferences
                    .as_ref()
                    .map(|value| value.close_behavior == settings::CloseBehavior::HideToTray)
                    .unwrap_or(true),
            )),
            active_notification_keys: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    /// Builds one complete snapshot. The mutex serializes SQLite scans and
    /// prevents manual, tray, and scheduled refreshes from racing each other.
    #[cfg(test)]
    pub(crate) fn collect(&self) -> DashboardSnapshot {
        self.collect_with_progress(|_| {})
    }

    pub(crate) fn collect_with_progress<F>(&self, mut on_progress: F) -> DashboardSnapshot
    where
        F: FnMut(snapshot::RefreshProgress),
    {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.collect_locked(&mut on_progress)
    }

    fn collect_locked<F>(&self, on_progress: &mut F) -> DashboardSnapshot
    where
        F: FnMut(snapshot::RefreshProgress),
    {
        let total_started = Instant::now();
        let codex_started = Instant::now();
        on_progress(snapshot::RefreshProgress::new(
            "codex",
            "正在读取 Codex 本机历史",
            0,
        ));
        let codex_usage = self.read_codex_local_usage(|processed, total| {
            on_progress(
                snapshot::RefreshProgress::new(
                    "codex",
                    if total > 0 {
                        format!("正在读取 Codex（{processed}/{total} 个文件）")
                    } else {
                        "正在读取 Codex 本机历史".to_owned()
                    },
                    0,
                )
                .with_files(
                    "Codex",
                    i64::try_from(processed).unwrap_or(i64::MAX),
                    i64::try_from(total).unwrap_or(i64::MAX),
                ),
            );
        });
        let codex_duration_ms = elapsed_milliseconds(codex_started);
        let sources_started = Instant::now();
        let device_usage = self.read_device_usage(codex_usage, |progress| {
            on_progress(
                snapshot::RefreshProgress::new(
                    "sources",
                    if progress.total_files > 0 {
                        format!(
                            "正在更新 {}（{}/{} 个文件）",
                            progress.source_label, progress.processed_files, progress.total_files
                        )
                    } else {
                        format!("正在检查 {} 数据源", progress.source_label)
                    },
                    progress.processed_sources,
                )
                .with_files(
                    progress.source_label,
                    progress.processed_files,
                    progress.total_files,
                ),
            );
        });
        let sources_duration_ms = elapsed_milliseconds(sources_started);
        on_progress(snapshot::RefreshProgress::new(
            "account",
            "正在读取账号额度",
            6,
        ));
        let account_started = Instant::now();
        let mut snapshot = snapshot::read_dashboard_snapshot();
        let account_duration_ms = elapsed_milliseconds(account_started);

        match settings::read_plan_renewal_at(&self.settings_path) {
            Ok(Some(value)) => {
                snapshot.plan_renewal_at = Some(value);
                snapshot.plan_renewal_source = Some("manual");
            }
            Ok(None) => {}
            Err(_) => append_message(&mut snapshot, "本机续费时间设置暂不可用。"),
        }

        match device_usage {
            Ok(summary) => {
                snapshot.last_successful_at = Some(summary.generated_at.clone());
                snapshot.source_health = summary
                    .sources
                    .iter()
                    .map(|source| snapshot::SourceHealthSummary {
                        id: source.id.clone(),
                        label: source.label.clone(),
                        status: source.status.clone(),
                        source_files: source.source_files,
                        indexed_events: source.indexed_events,
                        new_events: source.new_events,
                        skipped_records: source.skipped_records,
                        last_indexed_at: source.last_indexed_at.clone(),
                        message: source.message.clone(),
                    })
                    .collect();
                snapshot.device_usage = Some(summary);
            }
            Err(()) => append_message(&mut snapshot, "本机 AI 用量索引暂不可用。"),
        }
        let complete = snapshot::RefreshProgress::new("complete", "刷新完成", 6);
        snapshot.index_diagnostics = Some(snapshot::IndexDiagnostics {
            total_duration_ms: elapsed_milliseconds(total_started),
            codex_duration_ms,
            sources_duration_ms,
            account_duration_ms,
            databases: vec![
                snapshot::IndexDatabaseDiagnostic {
                    label: "Codex 索引",
                    size_bytes: database_footprint(&self.codex_database_path),
                },
                snapshot::IndexDatabaseDiagnostic {
                    label: "多来源索引",
                    size_bytes: database_footprint(&self.multi_source_database_path),
                },
            ],
        });
        snapshot.refresh_progress = Some(complete.clone());
        snapshot.refresh_schedule = Some(self.reset_refresh_schedule());
        on_progress(complete);
        self.store_cached_snapshot(&snapshot);
        snapshot
    }

    pub(crate) fn read_cached_dashboard_snapshot(&self) -> Option<serde_json::Value> {
        let bytes = fs::read(&self.snapshot_cache_path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn store_cached_snapshot(&self, snapshot: &DashboardSnapshot) {
        let Ok(bytes) = serde_json::to_vec(snapshot) else {
            return;
        };
        let _ = fs::create_dir_all(&self.app_data_directory);
        let _ = fs::write(&self.snapshot_cache_path, bytes);
    }

    pub(crate) fn index_maintenance_report(&self) -> IndexMaintenanceReport {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.index_maintenance_report_locked()
    }

    pub(crate) fn rebuild_indexes_with_progress<F>(
        &self,
        mut on_progress: F,
    ) -> Result<IndexRebuildResult, ()>
    where
        F: FnMut(snapshot::RefreshProgress),
    {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let backup_created_at = self.backup_and_remove_index_databases()?;
        on_progress(snapshot::RefreshProgress::new(
            "codex",
            "备份完成，正在重建派生索引",
            0,
        ));
        let snapshot = self.collect_locked(&mut on_progress);
        let report = self.index_maintenance_report_locked();
        Ok(IndexRebuildResult {
            backup_created_at,
            report,
            snapshot,
        })
    }

    fn index_maintenance_report_locked(&self) -> IndexMaintenanceReport {
        let databases = vec![
            inspect_codex_index(&self.codex_database_path),
            inspect_multi_source_index(&self.multi_source_database_path),
        ];
        let error_count = databases
            .iter()
            .filter(|database| database.status == "error")
            .count();
        let warning_count = databases
            .iter()
            .filter(|database| database.status == "missing")
            .count();
        let (overall_status, message) = if error_count > 0 {
            (
                "error",
                "检测到索引损坏或版本不匹配；可备份后重建派生索引。",
            )
        } else if warning_count > 0 {
            ("warning", "部分索引尚未建立；完成一次刷新后会自动创建。")
        } else {
            ("healthy", "两套派生索引均可读取，完整性和版本检查通过。")
        };
        IndexMaintenanceReport {
            checked_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            overall_status,
            databases,
            last_backup_at: settings::read_last_index_backup_at(&self.settings_path)
                .unwrap_or(None),
            message: message.to_owned(),
        }
    }

    fn backup_and_remove_index_databases(&self) -> Result<Option<String>, ()> {
        let database_paths = [&self.codex_database_path, &self.multi_source_database_path];
        if !database_paths
            .iter()
            .any(|path| database_family_exists(path))
        {
            return Ok(None);
        }
        let now = Utc::now();
        let created_at = now.to_rfc3339_opts(SecondsFormat::Secs, true);
        let backup_directory = self.app_data_directory.join("index-backups").join(format!(
            "{}-{:03}",
            now.format("%Y%m%dT%H%M%SZ"),
            now.timestamp_subsec_millis()
        ));
        fs::create_dir_all(&backup_directory).map_err(|_| ())?;
        for path in database_paths {
            copy_database_family(path, &backup_directory)?;
        }
        let manifest = serde_json::json!({
            "createdAt": created_at,
            "format": 1,
            "files": ["usage-index-v1.sqlite3", "usage-index-v2.sqlite3"],
            "privacy": "Derived indexes only; no source logs are included."
        });
        fs::write(
            backup_directory.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).map_err(|_| ())?,
        )
        .map_err(|_| ())?;
        for path in database_paths {
            remove_database_family(path)?;
        }
        let _ = settings::record_index_backup(&self.settings_path, &created_at);
        Ok(Some(created_at))
    }

    pub(crate) fn set_plan_renewal_at(
        &self,
        value: Option<&str>,
    ) -> Result<settings::PlanRenewalSetting, settings::SettingsError> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings::write_plan_renewal_at(&self.settings_path, value)
    }

    pub(crate) fn read_visual_preferences(
        &self,
    ) -> Result<settings::VisualPreferences, settings::SettingsError> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings::read_visual_preferences(&self.settings_path, &self.app_data_directory)
    }

    pub(crate) fn set_theme_preference(
        &self,
        theme_preference: settings::ThemePreference,
    ) -> Result<settings::VisualPreferences, settings::SettingsError> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings::set_theme_preference(
            &self.settings_path,
            &self.app_data_directory,
            theme_preference,
        )
    }

    pub(crate) fn select_background_image(
        &self,
    ) -> Result<settings::VisualPreferences, settings::SettingsError> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings::select_background_image(&self.settings_path, &self.app_data_directory)
    }

    pub(crate) fn clear_background_image(
        &self,
    ) -> Result<settings::VisualPreferences, settings::SettingsError> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings::clear_background_image(&self.settings_path, &self.app_data_directory)
    }

    pub(crate) fn read_app_preferences(
        &self,
        autostart_enabled: bool,
    ) -> Result<settings::AppPreferences, settings::SettingsError> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings::read_app_preferences(
            &self.settings_path,
            &self.app_data_directory,
            autostart_enabled,
        )
    }

    pub(crate) fn read_project_merge_rules(
        &self,
    ) -> Result<Vec<settings::ProjectMergeRule>, settings::SettingsError> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings::read_project_merge_rules(&self.settings_path)
    }

    pub(crate) fn set_project_merge_rules(
        &self,
        rules: &[settings::ProjectMergeRule],
    ) -> Result<Vec<settings::ProjectMergeRule>, settings::SettingsError> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings::write_project_merge_rules(&self.settings_path, rules)
    }

    pub(crate) fn read_pricing_settings(&self) -> Result<PricingSettings, settings::SettingsError> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings::read_pricing_settings(&self.settings_path)
    }

    pub(crate) fn set_pricing_settings(
        &self,
        pricing_settings: &PricingSettings,
    ) -> Result<PricingSettings, settings::SettingsError> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings::write_pricing_settings(&self.settings_path, pricing_settings)
    }

    pub(crate) fn set_app_preferences(
        &self,
        input: &settings::AppPreferencesInput,
        autostart_enabled: bool,
    ) -> Result<settings::AppPreferences, settings::SettingsError> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let preferences = settings::write_app_preferences(
            &self.settings_path,
            &self.app_data_directory,
            input,
            autostart_enabled,
        )?;
        self.refresh_interval_seconds
            .store(preferences.refresh_interval_minutes * 60, Ordering::Release);
        self.reset_refresh_schedule();
        self.hide_on_close.store(
            preferences.close_behavior == settings::CloseBehavior::HideToTray,
            Ordering::Release,
        );
        Ok(preferences)
    }

    pub(crate) fn hide_on_close(&self) -> bool {
        self.hide_on_close.load(Ordering::Acquire)
    }

    pub(crate) fn refresh_schedule(&self) -> snapshot::RefreshSchedule {
        let next = self.next_refresh_at_ms.load(Ordering::Acquire);
        snapshot::RefreshSchedule {
            next_refresh_at_ms: (next > 0).then_some(next),
            interval_seconds: self.refresh_interval_seconds.load(Ordering::Acquire),
        }
    }

    fn reset_refresh_schedule(&self) -> snapshot::RefreshSchedule {
        let interval_seconds = self
            .refresh_interval_seconds
            .load(Ordering::Acquire)
            .max(60);
        self.next_refresh_at_ms.store(
            epoch_milliseconds().saturating_add(interval_seconds * 1_000),
            Ordering::Release,
        );
        self.refresh_schedule()
    }

    pub(crate) fn read_usage_day_detail(&self, date: &str) -> Result<UsageDayDetail, ()> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut codex = IncrementalIndex::open(&self.codex_database_path).map_err(|_| ())?;
        codex.scan_codex_home(&self.codex_home).map_err(|_| ())?;
        let summary = codex.aggregate().map_err(|_| ())?;
        let all_events = codex.local_events().map_err(|_| ())?;
        let day_events = codex.local_events_for_date(date).map_err(|_| ())?;
        let mut multi = MultiSourceIndex::open(&self.multi_source_database_path).map_err(|_| ())?;
        // Re-scan the derived indexes before detail is read.  This only reads
        // source logs and keeps the day drawer consistent with Refresh.
        let pricing = settings::read_pricing_settings(&self.settings_path).unwrap_or_default();
        let _ =
            multi.collect_with_pricing(&self.user_home, Ok((summary, all_events, None)), &pricing);
        let mut tasks = multi.day_tasks_with_pricing(date, &pricing)?;
        tasks.extend(codex_day_tasks_with_pricing(day_events, &pricing));
        let threshold =
            settings::read_app_preferences(&self.settings_path, &self.app_data_directory, false)
                .map(|value| f64::from(value.cache_warning_percent) / 100.0)
                .unwrap_or(0.9);
        for task in &mut tasks {
            task.low_cache_hit = task.cache_hit_rate.is_some_and(|rate| rate < threshold);
        }
        Ok(build_day_detail(date.to_owned(), tasks))
    }

    pub(crate) fn maybe_notify<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        snapshot: &DashboardSnapshot,
    ) {
        let Ok(preferences) =
            settings::read_app_preferences(&self.settings_path, &self.app_data_directory, false)
        else {
            return;
        };
        if !preferences.notifications_enabled {
            self.active_notification_keys
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
            return;
        }
        if quiet_hours_active(
            preferences.quiet_hours_enabled,
            &preferences.quiet_hours_start,
            &preferences.quiet_hours_end,
        ) {
            self.active_notification_keys
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
            return;
        }

        let mut current = BTreeSet::new();
        let mut messages = Vec::new();
        for quota in &snapshot.quota_windows {
            if quota.remaining_percent < f64::from(preferences.quota_warning_percent) {
                let key = format!("quota:{}:{}", quota.key, preferences.quota_warning_percent);
                current.insert(key.clone());
                messages.push((
                    key,
                    format!("{}剩余 {:.0}%", quota.label, quota.remaining_percent),
                ));
            }
        }

        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        if let Ok(count) = self.cached_low_cache_task_count(
            &today,
            f64::from(preferences.cache_warning_percent) / 100.0,
        ) {
            if count > 0 {
                let key = format!("cache:{today}:{}", preferences.cache_warning_percent);
                current.insert(key.clone());
                messages.push((
                    key,
                    format!(
                        "今日有 {count} 个任务缓存命中率低于 {}%",
                        preferences.cache_warning_percent
                    ),
                ));
            }
        }

        let mut previous = self
            .active_notification_keys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let body = new_notification_body(&previous, &messages);
        *previous = current;
        drop(previous);
        if !body.is_empty()
            && app
                .notification()
                .builder()
                .title("AI Token 用量提醒")
                .body(&body)
                .show()
                .is_ok()
        {
            let _ = settings::record_notification(&self.settings_path, &body);
        }
    }

    pub(crate) fn send_test_notification<R: Runtime>(&self, app: &AppHandle<R>) -> Result<(), ()> {
        let reason = "测试通知：系统通知链路可用";
        app.notification()
            .builder()
            .title("AI Token 用量监控")
            .body(reason)
            .show()
            .map_err(|_| ())?;
        settings::record_notification(&self.settings_path, reason).map_err(|_| ())
    }

    pub(crate) fn request_event_refresh<R: Runtime>(&self, app: AppHandle<R>) {
        if self.event_refresh_in_flight.swap(true, Ordering::AcqRel) {
            return;
        }
        let pending_schedule = self.reset_refresh_schedule();
        let _ = app.emit(REFRESH_SCHEDULE_EVENT, pending_schedule);
        let runtime = self.clone();
        thread::spawn(move || {
            let progress_app = app.clone();
            let snapshot = runtime.collect_with_progress(|progress| {
                let _ = progress_app.emit(PROGRESS_EVENT, progress);
            });
            if !runtime.shutdown.load(Ordering::Acquire) {
                update_tray_tooltip(&app, &snapshot);
                runtime.maybe_notify(&app, &snapshot);
                let _ = app.emit(SNAPSHOT_EVENT, snapshot);
            }
            runtime
                .event_refresh_in_flight
                .store(false, Ordering::Release);
        });
    }

    pub(crate) fn start_scheduler<R: Runtime>(&self, app: AppHandle<R>) {
        let runtime = self.clone();
        thread::spawn(move || loop {
            if runtime.wait_or_shutdown(Duration::from_secs(1)) {
                return;
            }
            let next_refresh_at_ms = runtime.next_refresh_at_ms.load(Ordering::Acquire);
            if next_refresh_at_ms > 0 && epoch_milliseconds() >= next_refresh_at_ms {
                runtime.request_event_refresh(app.clone());
            }
        });
    }

    pub(crate) fn stop(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    fn read_codex_local_usage<F>(
        &self,
        on_progress: F,
    ) -> Result<(LocalUsageSummary, Vec<LocalUsageEvent>, Option<i64>), ()>
    where
        F: FnMut(usize, usize),
    {
        let mut index = IncrementalIndex::open(&self.codex_database_path).map_err(|_| ())?;
        let scan = index
            .scan_codex_home_with_progress(&self.codex_home, on_progress)
            .map_err(|_| ())?;
        let summary = index.aggregate().map_err(|_| ())?;
        let events = index.local_events().map_err(|_| ())?;
        Ok((summary, events, Some(scan.indexed_events_added)))
    }

    fn read_device_usage<F>(
        &self,
        codex_usage: Result<(LocalUsageSummary, Vec<LocalUsageEvent>, Option<i64>), ()>,
        on_source: F,
    ) -> Result<DeviceUsageSummary, ()>
    where
        F: FnMut(SourceScanProgress),
    {
        let mut index = MultiSourceIndex::open(&self.multi_source_database_path).map_err(|_| ())?;
        let pricing = settings::read_pricing_settings(&self.settings_path).unwrap_or_default();
        Ok(index.collect_with_progress_and_pricing(
            &self.user_home,
            codex_usage,
            &pricing,
            on_source,
        ))
    }

    fn cached_low_cache_task_count(&self, date: &str, threshold: f64) -> Result<i64, ()> {
        let codex = IncrementalIndex::open(&self.codex_database_path).map_err(|_| ())?;
        let day_events = codex.local_events_for_date(date).map_err(|_| ())?;
        let multi = MultiSourceIndex::open(&self.multi_source_database_path).map_err(|_| ())?;
        let mut tasks = multi.day_tasks(date)?;
        tasks.extend(codex_day_tasks(day_events));
        Ok(tasks
            .iter()
            .filter(|task| task.cache_hit_rate.is_some_and(|rate| rate < threshold))
            .count() as i64)
    }

    fn wait_or_shutdown(&self, duration: Duration) -> bool {
        let one_second = Duration::from_secs(1);
        let mut remaining = duration;
        while !remaining.is_zero() {
            if self.shutdown.load(Ordering::Acquire) {
                return true;
            }
            let step = remaining.min(one_second);
            thread::sleep(step);
            remaining = remaining.saturating_sub(step);
        }
        self.shutdown.load(Ordering::Acquire)
    }
}

fn elapsed_milliseconds(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn epoch_milliseconds() -> u64 {
    u64::try_from(Utc::now().timestamp_millis()).unwrap_or(0)
}

fn database_footprint(path: &std::path::Path) -> u64 {
    let base = path.metadata().map(|value| value.len()).unwrap_or(0);
    ["-wal", "-shm"].iter().fold(base, |total, suffix| {
        let companion = PathBuf::from(format!("{}{suffix}", path.to_string_lossy()));
        total.saturating_add(companion.metadata().map(|value| value.len()).unwrap_or(0))
    })
}

fn inspect_codex_index(path: &Path) -> IndexDatabaseHealth {
    let expected = SCHEMA_VERSION;
    if !path.exists() {
        return missing_index_health("Codex 索引", expected);
    }
    let size_bytes = database_footprint(path);
    let Ok(connection) = open_read_only(path) else {
        return error_index_health("Codex 索引", size_bytes, None, expected, "unavailable");
    };
    if !quick_check_ok(&connection) {
        return error_index_health("Codex 索引", size_bytes, None, expected, "failed");
    }
    let application_id = connection
        .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
        .ok();
    let schema_version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .ok();
    let parser_version = connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = 'parser_semantics_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok());
    let valid = application_id == Some(APPLICATION_ID)
        && schema_version == Some(expected)
        && parser_version == Some(PARSER_SEMANTICS_VERSION);
    if valid {
        healthy_index_health("Codex 索引", size_bytes, schema_version, expected)
    } else {
        error_index_health("Codex 索引", size_bytes, schema_version, expected, "ok")
    }
}

fn inspect_multi_source_index(path: &Path) -> IndexDatabaseHealth {
    let expected = MULTI_SOURCE_SCHEMA_VERSION;
    if !path.exists() {
        return missing_index_health("多来源索引", expected);
    }
    let size_bytes = database_footprint(path);
    let Ok(connection) = open_read_only(path) else {
        return error_index_health("多来源索引", size_bytes, None, expected, "unavailable");
    };
    if !quick_check_ok(&connection) {
        return error_index_health("多来源索引", size_bytes, None, expected, "failed");
    }
    let schema_version = connection
        .query_row(
            "SELECT value FROM source_index_meta WHERE key = 'schema-version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok());
    if schema_version == Some(expected) {
        healthy_index_health("多来源索引", size_bytes, schema_version, expected)
    } else {
        error_index_health("多来源索引", size_bytes, schema_version, expected, "ok")
    }
}

fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(connection)
}

fn quick_check_ok(connection: &Connection) -> bool {
    connection
        .query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))
        .is_ok_and(|value| value.eq_ignore_ascii_case("ok"))
}

fn missing_index_health(label: &'static str, expected_schema_version: i64) -> IndexDatabaseHealth {
    IndexDatabaseHealth {
        label,
        status: "missing",
        integrity: "unavailable",
        size_bytes: 0,
        schema_version: None,
        expected_schema_version,
    }
}

fn healthy_index_health(
    label: &'static str,
    size_bytes: u64,
    schema_version: Option<i64>,
    expected_schema_version: i64,
) -> IndexDatabaseHealth {
    IndexDatabaseHealth {
        label,
        status: "healthy",
        integrity: "ok",
        size_bytes,
        schema_version,
        expected_schema_version,
    }
}

fn error_index_health(
    label: &'static str,
    size_bytes: u64,
    schema_version: Option<i64>,
    expected_schema_version: i64,
    integrity: &'static str,
) -> IndexDatabaseHealth {
    IndexDatabaseHealth {
        label,
        status: "error",
        integrity,
        size_bytes,
        schema_version,
        expected_schema_version,
    }
}

fn family_member(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn database_family_exists(path: &Path) -> bool {
    ["", "-wal", "-shm"]
        .iter()
        .any(|suffix| family_member(path, suffix).exists())
}

fn copy_database_family(path: &Path, backup_directory: &Path) -> Result<(), ()> {
    let file_name = path.file_name().ok_or(())?;
    for suffix in ["", "-wal", "-shm"] {
        let source = family_member(path, suffix);
        if !source.exists() {
            continue;
        }
        let mut target_name = file_name.to_os_string();
        target_name.push(suffix);
        fs::copy(&source, backup_directory.join(target_name)).map_err(|_| ())?;
    }
    Ok(())
}

fn remove_database_family(path: &Path) -> Result<(), ()> {
    for suffix in ["", "-wal", "-shm"] {
        let member = family_member(path, suffix);
        if member.exists() {
            fs::remove_file(member).map_err(|_| ())?;
        }
    }
    Ok(())
}

fn quiet_hours_active(enabled: bool, start: &str, end: &str) -> bool {
    use chrono::Timelike;

    let now = chrono::Local::now().time();
    quiet_hours_active_at(enabled, start, end, now.hour() * 60 + now.minute())
}

fn quiet_hours_active_at(enabled: bool, start: &str, end: &str, now: u32) -> bool {
    use chrono::Timelike;

    if !enabled {
        return false;
    }
    let parse_minutes = |value: &str| {
        chrono::NaiveTime::parse_from_str(value, "%H:%M")
            .ok()
            .map(|time| time.hour() * 60 + time.minute())
    };
    let (Some(start), Some(end)) = (parse_minutes(start), parse_minutes(end)) else {
        return false;
    };
    if start == end {
        true
    } else if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

fn new_notification_body(previous: &BTreeSet<String>, messages: &[(String, String)]) -> String {
    messages
        .iter()
        .filter(|(key, _)| !previous.contains(key))
        .map(|(_, message)| message.as_str())
        .collect::<Vec<_>>()
        .join("；")
}

fn append_message(snapshot: &mut DashboardSnapshot, addition: &str) {
    snapshot.message = Some(match snapshot.message.take() {
        Some(message) => format!("{message} {addition}"),
        None => addition.to_owned(),
    });
}

pub(crate) fn update_tray_tooltip<R: Runtime>(app: &AppHandle<R>, snapshot: &DashboardSnapshot) {
    let mut lines = vec!["AI Token 用量监控".to_owned()];
    for quota in snapshot.quota_windows.iter().take(2) {
        let label = if quota.label.is_empty() {
            "额度"
        } else {
            quota.label.as_str()
        };
        lines.push(format!("{label}：剩余 {:.0}%", quota.remaining_percent));
    }
    if let Some(fetched_at) = snapshot.fetched_at.as_deref() {
        if let Ok(value) = chrono::DateTime::parse_from_rfc3339(fetched_at) {
            lines.push(format!(
                "更新于 {}",
                value.with_timezone(&chrono::Local).format("%H:%M")
            ));
        }
    }
    if let Some(tray) = app.tray_by_id(crate::tray::TRAY_ID) {
        let _ = tray.set_tooltip(Some(lines.join("\n")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_interrupts_the_scheduler_wait() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        runtime.stop();
        assert!(runtime.wait_or_shutdown(Duration::from_secs(2)));
    }

    #[test]
    fn refresh_schedule_uses_the_runtime_interval_and_a_future_deadline() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        let before = epoch_milliseconds();
        let schedule = runtime.refresh_schedule();
        assert_eq!(schedule.interval_seconds, 60);
        assert!(schedule
            .next_refresh_at_ms
            .is_some_and(|value| value >= before + 59_000));
        let preferences = settings::AppPreferencesInput {
            refresh_interval_minutes: 5,
            close_behavior: settings::CloseBehavior::HideToTray,
            quota_warning_percent: 20,
            cache_warning_percent: 90,
            notifications_enabled: true,
            quiet_hours_enabled: false,
            quiet_hours_start: "22:00".to_owned(),
            quiet_hours_end: "08:00".to_owned(),
        };
        let changed_at = epoch_milliseconds();
        runtime
            .set_app_preferences(&preferences, false)
            .expect("change refresh interval");
        let changed = runtime.refresh_schedule();
        assert_eq!(changed.interval_seconds, 300);
        assert!(changed
            .next_refresh_at_ms
            .is_some_and(|value| value >= changed_at + 299_000));
    }

    #[test]
    fn quiet_hours_support_cross_midnight_and_exact_boundaries() {
        assert!(quiet_hours_active_at(true, "22:00", "08:00", 23 * 60));
        assert!(quiet_hours_active_at(true, "22:00", "08:00", 7 * 60 + 59));
        assert!(!quiet_hours_active_at(true, "22:00", "08:00", 8 * 60));
        assert!(!quiet_hours_active_at(true, "22:00", "08:00", 12 * 60));
        assert!(!quiet_hours_active_at(false, "22:00", "08:00", 23 * 60));
    }

    #[test]
    fn notification_messages_only_include_newly_active_conditions() {
        let previous = BTreeSet::from(["quota:short:20".to_owned()]);
        let messages = vec![
            ("quota:short:20".to_owned(), "短周期额度剩余 12%".to_owned()),
            (
                "cache:2026-08-31:90".to_owned(),
                "今日有 2 个低缓存任务".to_owned(),
            ),
        ];
        assert_eq!(
            new_notification_body(&previous, &messages),
            "今日有 2 个低缓存任务"
        );
    }

    #[test]
    fn index_self_check_reports_versions_and_detects_corruption() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        IncrementalIndex::open(&runtime.codex_database_path).expect("create Codex index");
        MultiSourceIndex::open(&runtime.multi_source_database_path)
            .expect("create multi-source index");

        let healthy = runtime.index_maintenance_report();
        assert_eq!(healthy.overall_status, "healthy");
        assert!(healthy
            .databases
            .iter()
            .all(|database| database.status == "healthy"));

        fs::write(
            &runtime.multi_source_database_path,
            b"not a SQLite database",
        )
        .expect("damage only the temporary derived index");
        let damaged = runtime.index_maintenance_report();
        assert_eq!(damaged.overall_status, "error");
        assert_eq!(damaged.databases[1].status, "error");
    }

    #[test]
    fn maintenance_backup_contains_only_derived_indexes_before_removal() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let codex_home = temporary.path().join("codex");
        let source_log = codex_home.join("sessions").join("source.jsonl");
        fs::create_dir_all(source_log.parent().expect("source parent"))
            .expect("create source folder");
        fs::write(&source_log, b"source-history-stays-read-only").expect("write source sentinel");
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            codex_home,
            temporary.path().join("home"),
        );
        IncrementalIndex::open(&runtime.codex_database_path).expect("create Codex index");
        MultiSourceIndex::open(&runtime.multi_source_database_path)
            .expect("create multi-source index");

        let created_at = runtime
            .backup_and_remove_index_databases()
            .expect("backup succeeds")
            .expect("backup timestamp");
        assert!(!runtime.codex_database_path.exists());
        assert!(!runtime.multi_source_database_path.exists());
        assert_eq!(
            fs::read(&source_log).expect("source remains readable"),
            b"source-history-stays-read-only"
        );
        let backup_root = runtime.app_data_directory.join("index-backups");
        let backup = fs::read_dir(&backup_root)
            .expect("backup root")
            .next()
            .expect("one backup")
            .expect("backup entry")
            .path();
        assert!(backup.join("usage-index-v1.sqlite3").is_file());
        assert!(backup.join("usage-index-v2.sqlite3").is_file());
        assert!(backup.join("manifest.json").is_file());
        assert_eq!(
            settings::read_last_index_backup_at(&runtime.settings_path)
                .expect("read backup timestamp")
                .as_deref(),
            Some(created_at.as_str())
        );
    }

    #[test]
    #[ignore = "scans the current machine's complete Codex rollout history"]
    fn live_runtime_builds_a_private_multisource_summary_without_duplicates() {
        let codex_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE").map(|path| PathBuf::from(path).join(".codex"))
            })
            .expect("Codex home is discoverable");
        assert!(codex_home.join("sessions").is_dir());

        let temporary = tempfile::tempdir().expect("temp dir");
        let user_home = codex_home
            .parent()
            .expect("Codex home has a user parent")
            .to_path_buf();
        let runtime =
            UsageRuntime::new(temporary.path().join("data"), codex_home.clone(), user_home);
        let snapshot = runtime.collect();
        let second_snapshot = runtime.collect();
        assert_eq!(snapshot.status, "ready");
        let local = snapshot.device_usage.as_ref().expect("device summary");
        let second_local = second_snapshot
            .device_usage
            .as_ref()
            .expect("second device summary");
        let codex = local
            .sources
            .iter()
            .find(|source| source.id == "codex")
            .expect("Codex source");
        assert!(codex.source_files > 0);
        assert!(codex.indexed_events > 0);
        for source_id in [
            "codex",
            "claude-code",
            "opencode",
            "workbuddy",
            "workbuddy-ai",
            "cursor",
        ] {
            let first = local
                .sources
                .iter()
                .find(|source| source.id == source_id)
                .expect("expected source status");
            let second = second_local
                .sources
                .iter()
                .find(|source| source.id == source_id)
                .expect("expected source status on second scan");
            assert!(
                second.indexed_events >= first.indexed_events,
                "{source_id} cannot lose events during a live incremental refresh"
            );
            assert!(
                second.total.total_tokens >= first.total.total_tokens,
                "{source_id} cannot decrease its live derived total"
            );
            println!(
                "source={source_id} status={} files={} events={} total={}",
                first.status, first.source_files, first.indexed_events, first.total.total_tokens
            );
        }

        let serialized = serde_json::to_string(&snapshot).expect("snapshot serializes");
        assert!(!serialized.contains(&codex_home.to_string_lossy().to_string()));
    }
}
