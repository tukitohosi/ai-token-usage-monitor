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
    local_generation: Arc<AtomicU64>,
    latest_snapshot: Arc<Mutex<Option<DashboardSnapshot>>>,
    account_cache: Arc<Mutex<crate::account_sync::AccountCache>>,
    account_task: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    local_only: Arc<AtomicBool>,
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
        let local_only = preferences.as_ref().is_some_and(|p| p.local_only);
        let mut account_cache: crate::account_sync::AccountCache =
            fs::read(app_data_directory.join("account-cache-v1.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
        account_cache.invalidate_credentials(credential_fingerprint(&codex_home));
        account_cache.mark_stale(if local_only { "localOnly" } else { "idle" });
        Self {
            latest_snapshot: Arc::new(Mutex::new(None)),
            account_cache: Arc::new(Mutex::new(account_cache)),
            account_task: Arc::new(Mutex::new(None)),
            local_only: Arc::new(AtomicBool::new(local_only)),
            app_data_directory: app_data_directory.clone(),
            codex_database_path: app_data_directory.join("usage-index-v1.sqlite3"),
            multi_source_database_path: app_data_directory.join("usage-index-v2.sqlite3"),
            settings_path,
            snapshot_cache_path: app_data_directory.join("dashboard-snapshot-v1.json"),
            codex_home,
            user_home,
            refresh_gate: Arc::new(Mutex::new(())),
            local_generation: Arc::new(AtomicU64::new(0)),
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

    pub(crate) fn emit_local_progress<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        progress: snapshot::RefreshProgress,
    ) {
        self.publish_local_progress(
            progress,
            |snapshot| { let _ = app.emit(SNAPSHOT_EVENT, snapshot); },
            |progress| { let _ = app.emit(PROGRESS_EVENT, progress); },
        );
    }

    fn publish_local_progress<S, P>(
        &self,
        progress: snapshot::RefreshProgress,
        emit_snapshot: S,
        emit_progress: P,
    ) where
        S: FnOnce(DashboardSnapshot),
        P: FnOnce(snapshot::RefreshProgress),
    {
        if progress.phase == "complete" {
            // collect_locked still holds refresh_gate here, so the next scan's
            // progress cannot overtake this committed snapshot and completion.
            let snapshot = self
                .latest_snapshot
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if let Some(snapshot) = snapshot {
                emit_snapshot(snapshot);
            }
        }
        emit_progress(progress);
    }

    pub(crate) fn collect_with_progress<F>(&self, mut on_progress: F) -> DashboardSnapshot
    where
        F: FnMut(snapshot::RefreshProgress),
    {
        let generation = self.local_generation.load(Ordering::Acquire);
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if generation != self.local_generation.load(Ordering::Acquire) {
            if let Some(snapshot) = self
                .latest_snapshot
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
            {
                return snapshot;
            }
        }
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
        let account_duration_ms = 0;
        let mut snapshot = DashboardSnapshot::empty("ready", Utc::now().to_rfc3339(), "");
        snapshot.message = None;

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
            Err(()) => {
                snapshot.status = "error";
                append_message(&mut snapshot, "本机 AI 用量索引暂不可用。");
            }
        }
        let complete = snapshot::RefreshProgress::new("complete", "本机统计已更新", 6);
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
        let mut latest = self
            .latest_snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut account = self.account_cache.lock().unwrap_or_else(|p| p.into_inner());
        self.invalidate_account_cache_for_local_read(&mut account);
        if self.local_only.load(Ordering::Acquire) {
            account.mark_stale("localOnly");
        }
        account.apply(&mut snapshot);
        snapshot.refresh_schedule = Some(self.reset_refresh_schedule());
        snapshot.revision = latest.as_ref().map_or(1, |s| s.revision + 1);
        self.store_cached_snapshot(&snapshot);
        *latest = Some(snapshot.clone());
        self.local_generation.fetch_add(1, Ordering::Release);
        drop(account);
        drop(latest);
        on_progress(complete);
        snapshot
    }

    fn invalidate_account_cache_for_local_read(
        &self,
        account: &mut crate::account_sync::AccountCache,
    ) {
        // Keep transport state when clearing data that has no verifiable owner.
        let fingerprint = credential_fingerprint(&self.codex_home);
        let status = (account.sync.status == "syncing"
            || (fingerprint.is_none() && account.credential_fingerprint.is_none()))
            .then(|| account.sync.status.clone());
        account.invalidate_credentials(fingerprint);
        if let Some(status) = status {
            account.mark_stale(&status);
        }
    }

    pub(crate) fn read_cached_dashboard_snapshot(&self) -> Option<serde_json::Value> {
        let bytes = fs::read(&self.snapshot_cache_path).ok()?;
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let object = value.as_object_mut()?;
        // Legacy cache account data has no verifiable ownership/freshness.
        let mut account = self.account_cache.lock().unwrap_or_else(|p| p.into_inner());
        self.invalidate_account_cache_for_local_read(&mut account);
        let mut sanitized = DashboardSnapshot::empty("ready", Utc::now().to_rfc3339(), "");
        account.apply(&mut sanitized);
        let fields = serde_json::to_value(sanitized).ok()?;
        for key in [
            "quotaWindows",
            "resetCredits",
            "accountUsage",
            "codexVersion",
            "accountSync",
        ] {
            object.insert(key.to_owned(), fields[key].clone());
        }
        object.insert("revision".into(), 0.into());
        Some(value)
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
        let backup_created_at = self.backup_index_databases()?;
        // Only invalidate scan stamps. Historical events remain available even
        // when their source was archived, moved, or is temporarily unreadable.
        IncrementalIndex::open(&self.codex_database_path)
            .and_then(|mut index| index.invalidate_scan_state_for_rebuild())
            .map_err(|_| ())?;
        MultiSourceIndex::open(&self.multi_source_database_path)
            .and_then(|mut index| index.invalidate_scan_state_for_rebuild())
            .map_err(|_| ())?;
        on_progress(snapshot::RefreshProgress::new(
            "codex",
            "一致性备份完成，正在保留历史并重新扫描来源",
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
                "检测到索引损坏或版本不匹配；请保留历史备份后修复，重建不会清空历史。",
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

    fn backup_index_databases(&self) -> Result<Option<String>, ()> {
        let database_paths = [&self.codex_database_path, &self.multi_source_database_path];
        if !database_paths
            .iter()
            .any(|path| database_family_exists(path))
        {
            return Ok(None);
        }
        let now = Utc::now();
        let created_at = now.to_rfc3339_opts(SecondsFormat::Secs, true);
        let backup_root = self.app_data_directory.join("index-backups");
        fs::create_dir_all(&backup_root).map_err(|_| ())?;
        let backup_directory = backup_root.join(now.format("%Y%m%dT%H%M%S%.9fZ").to_string());
        fs::create_dir(&backup_directory).map_err(|_| ())?;
        for path in database_paths {
            backup_database_snapshot(path, &backup_directory)?;
        }
        let manifest = serde_json::json!({
            "createdAt": created_at,
            "format": 2,
            "backupMethod": "sqlite-online-backup",
            "files": ["usage-index-v1.sqlite3", "usage-index-v2.sqlite3"],
            "privacy": "Usage history indexes only; no source logs are included."
        });
        fs::write(
            backup_directory.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).map_err(|_| ())?,
        )
        .map_err(|_| ())?;
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
        let previous = self
            .local_only
            .swap(preferences.local_only, Ordering::AcqRel);
        if previous != preferences.local_only {
            self.cancel_account_task();
            self.account_cache
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .mark_stale(if preferences.local_only {
                    "localOnly"
                } else {
                    "idle"
                });
        }
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

    pub(crate) fn read_usage_export(
        &self,
        query: crate::usage_export::UsageExportQuery,
    ) -> Result<crate::usage_export::UsageExportReport, String> {
        let _refresh_guard = self
            .refresh_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Health is explicitly last-known; day-detail reads can update indexes
        // independently of the last dashboard snapshot. Never imply a revision match.
        let snapshot = self
            .latest_snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|value| serde_json::to_value(value).ok())
            .or_else(|| {
                fs::read(&self.snapshot_cache_path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            });
        crate::usage_export::read_report(
            query,
            &self.codex_database_path,
            &self.multi_source_database_path,
            &self.settings_path,
            snapshot.as_ref(),
        )
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
        for quota in snapshot.quota_windows.iter().filter(|_| {
            snapshot
                .account_sync
                .as_ref()
                .is_some_and(|s| !s.quota.stale)
        }) {
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
        if snapshot
            .account_sync
            .as_ref()
            .map_or(true, |s| s.quota.stale)
        {
            current.extend(
                previous
                    .iter()
                    .filter(|key| key.starts_with("quota:"))
                    .cloned(),
            );
        }
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
        self.emit_local_progress(
            &app,
            snapshot::RefreshProgress::new("codex", "正在读取本机与账号用量", 0),
        );
        let pending_schedule = self.reset_refresh_schedule();
        let _ = app.emit(REFRESH_SCHEDULE_EVENT, pending_schedule);
        let runtime = self.clone();
        thread::spawn(move || {
            runtime.request_account_refresh(app.clone());
            let progress_app = app.clone();
            let snapshot = runtime.collect_with_progress(|progress| {
                runtime.emit_local_progress(&progress_app, progress);
            });
            if !runtime.shutdown.load(Ordering::Acquire) {
                update_tray_tooltip(&app, &snapshot);
                runtime.maybe_notify(&app, &snapshot);
            }
            runtime
                .event_refresh_in_flight
                .store(false, Ordering::Release);
        });
    }

    fn cancel_account_task(&self) {
        if let Some(cancelled) = self
            .account_task
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            cancelled.store(true, Ordering::Release);
        }
    }

    fn cache_account_snapshot(&self) -> Option<DashboardSnapshot> {
        let mut latest = self
            .latest_snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let account = self.account_cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(snapshot) = latest.as_mut() {
            account.apply(snapshot);
            snapshot.refresh_schedule = Some(self.refresh_schedule());
            snapshot.revision += 1;
            self.store_cached_snapshot(snapshot);
        }
        if let Ok(bytes) = serde_json::to_vec(&*account) {
            let _ = fs::create_dir_all(&self.app_data_directory);
            let _ = fs::write(self.app_data_directory.join("account-cache-v1.json"), bytes);
        }
        latest.clone()
    }

    pub(crate) fn publish_account<R: Runtime>(&self, app: &AppHandle<R>) {
        if let Some(snapshot) = self.cache_account_snapshot() {
            let _ = app.emit(SNAPSHOT_EVENT, &snapshot);
            update_tray_tooltip(app, &snapshot);
        }
    }

    pub(crate) fn request_account_refresh<R: Runtime>(&self, app: AppHandle<R>) {
        if self.local_only.load(Ordering::Acquire) || self.shutdown.load(Ordering::Acquire) {
            return;
        }
        let mut task = self.account_task.lock().unwrap_or_else(|p| p.into_inner());
        if task.is_some() {
            return;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        *task = Some(cancelled.clone());
        let fingerprint = credential_fingerprint(&self.codex_home);
        {
            let mut cache = self.account_cache.lock().unwrap_or_else(|p| p.into_inner());
            cache.invalidate_credentials(fingerprint.clone());
            cache.mark_stale("syncing");
        }
        self.publish_account(&app);
        let runtime = self.clone();
        thread::spawn(move || {
            let result = snapshot::read_dashboard_snapshot(cancelled.clone());
            let mut task = runtime
                .account_task
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if !task
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, &cancelled))
            {
                return;
            }
            if !cancelled.load(Ordering::Acquire)
                && !runtime.local_only.load(Ordering::Acquire)
                && !runtime.shutdown.load(Ordering::Acquire)
            {
                {
                    let mut cache = runtime
                        .account_cache
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if credential_fingerprint(&runtime.codex_home) == fingerprint {
                        cache.merge(&result);
                    } else {
                        cache.invalidate_credentials(credential_fingerprint(&runtime.codex_home));
                    }
                }
                runtime.reset_refresh_schedule();
                runtime.publish_account(&app);
                let latest = runtime
                    .latest_snapshot
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                if let Some(snapshot) = latest {
                    runtime.maybe_notify(&app, &snapshot);
                }
            }
            *task = None;
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
        self.cancel_account_task();
    }

    fn read_codex_local_usage<F>(
        &self,
        on_progress: F,
    ) -> Result<(LocalUsageSummary, Vec<LocalUsageEvent>, Option<i64>), ()>
    where
        F: FnMut(usize, usize),
    {
        let mut index = IncrementalIndex::open(&self.codex_database_path).map_err(|_| ())?;
        let scan = index.scan_codex_home_with_progress(&self.codex_home, on_progress);
        let mut summary = index.aggregate().map_err(|_| ())?;
        let new_events = match scan {
            Ok(scan) => Some(scan.indexed_events_added),
            Err(_) => {
                summary.warnings.push(
                    "本轮部分 Codex 日志未能完整读取，当前展示已成功记录的历史用量；稍后刷新将重试。".to_owned(),
                );
                None
            }
        };
        let events = index.local_events().map_err(|_| ())?;
        Ok((summary, events, new_events))
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

fn credential_fingerprint(codex_home: &Path) -> Option<String> {
    fs::read(codex_home.join("auth.json"))
        .ok()
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
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

fn backup_database_snapshot(path: &Path, backup_directory: &Path) -> Result<(), ()> {
    if !path.exists() {
        // An orphaned WAL must not be silently treated as an empty database.
        return if database_family_exists(path) {
            Err(())
        } else {
            Ok(())
        };
    }
    let file_name = path.file_name().ok_or(())?;
    let target = backup_directory.join(file_name);
    let source = open_read_only(path).map_err(|_| ())?;
    source
        .backup(rusqlite::DatabaseName::Main, &target, None)
        .map_err(|_| ())?;
    let backup = open_read_only(&target).map_err(|_| ())?;
    if !quick_check_ok(&backup) {
        return Err(());
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
    fn complete_progress_observes_the_committed_snapshot_without_account_or_snapshot_locks() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        let pending = Arc::new(AtomicBool::new(false));
        *runtime.account_task.lock().unwrap() = Some(pending.clone());
        runtime.account_cache.lock().unwrap().mark_stale("syncing");
        let mut completed_revision = None;
        let snapshot = runtime.collect_with_progress(|progress| {
            if progress.phase != "complete" {
                return;
            }
            let latest = runtime.latest_snapshot.try_lock().expect("snapshot lock released");
            let latest = latest.as_ref().expect("latest snapshot committed");
            assert!(runtime.account_cache.try_lock().is_ok(), "account lock released");
            assert_eq!(runtime.local_generation.load(Ordering::Acquire), 1);
            let cached: serde_json::Value = serde_json::from_slice(
                &fs::read(&runtime.snapshot_cache_path).expect("snapshot persisted"),
            ).unwrap();
            assert_eq!(cached["revision"], latest.revision);
            assert_eq!(cached["refreshProgress"]["phase"], "complete");
            let cached = runtime.read_cached_dashboard_snapshot().unwrap();
            assert_eq!(cached["accountSync"]["status"], "syncing");
            assert!(latest.device_usage.is_some());
            completed_revision = Some(latest.revision);
        });
        assert_eq!(completed_revision, Some(snapshot.revision));
        assert_eq!(snapshot.account_sync.unwrap().status, "syncing");
        assert!(runtime.account_task.lock().unwrap().is_some());
        assert!(!pending.load(Ordering::Acquire));
    }

    #[test]
    fn the_next_local_scan_cannot_overtake_snapshot_and_completion_publication() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut second_scan = None;
        runtime.collect_with_progress(|progress| {
            if progress.phase != "complete" {
                return;
            }
            runtime.publish_local_progress(
                progress,
                |snapshot| {
                    assert_eq!(snapshot.revision, 1);
                    assert!(runtime.refresh_gate.try_lock().is_err());
                    events.lock().unwrap().push("first snapshot");
                    let runtime = runtime.clone();
                    let events = events.clone();
                    let (attempted, receive_attempt) = std::sync::mpsc::channel();
                    second_scan = Some(thread::spawn(move || {
                        attempted.send(()).unwrap();
                        let mut started = false;
                        runtime.collect_with_progress(|_| {
                            if !started {
                                events.lock().unwrap().push("second started");
                                started = true;
                            }
                        })
                    }));
                    receive_attempt.recv_timeout(Duration::from_secs(5)).unwrap();
                },
                |progress| {
                    assert_eq!(progress.phase, "complete");
                    events.lock().unwrap().push("first complete");
                },
            );
        });
        assert_eq!(second_scan.unwrap().join().unwrap().revision, 2);
        assert_eq!(
            *events.lock().unwrap(),
            vec!["first snapshot", "first complete", "second started"],
        );
    }

    #[test]
    fn a_local_scan_preserves_an_earlier_account_failure_without_credentials() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        for status in ["unauthenticated", "offline"] {
            runtime.account_cache.lock().unwrap().mark_stale(status);
            let snapshot = runtime.collect();
            assert_eq!(snapshot.account_sync.unwrap().status, status);
            let cached = runtime.read_cached_dashboard_snapshot().unwrap();
            assert_eq!(cached["accountSync"]["status"], status);
        }
    }

    #[test]
    fn local_credential_validation_preserves_fresh_data_only_for_unchanged_credentials() {
        let temporary = tempfile::tempdir().unwrap();
        let codex_home = temporary.path().join("codex");
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(codex_home.join("auth.json"), b"credential-a").unwrap();
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            codex_home.clone(),
            temporary.path().join("home"),
        );
        let mut account = runtime.account_cache.lock().unwrap();
        account.sync.status = "ready".into();
        account.sync.quota.stale = false;
        account.sync.usage.stale = false;
        runtime.invalidate_account_cache_for_local_read(&mut account);
        assert_eq!(account.sync.status, "ready");
        assert!(!account.sync.quota.stale && !account.sync.usage.stale);

        fs::write(codex_home.join("auth.json"), b"credential-b").unwrap();
        runtime.invalidate_account_cache_for_local_read(&mut account);
        assert_eq!(account.sync.status, "idle");
        account.mark_stale("syncing");
        fs::write(codex_home.join("auth.json"), b"credential-c").unwrap();
        runtime.invalidate_account_cache_for_local_read(&mut account);
        assert_eq!(account.sync.status, "syncing");
    }

    #[test]
    fn the_last_finishing_local_or_account_stage_supplies_the_published_refresh_schedule() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        // Account finishes first; the following local scan sets a fresh deadline.
        runtime.reset_refresh_schedule();
        assert!(runtime.cache_account_snapshot().is_none());
        runtime.next_refresh_at_ms.store(1, Ordering::Release);
        let before_local = epoch_milliseconds();
        let local = runtime.collect();
        let local_deadline = local.refresh_schedule.unwrap().next_refresh_at_ms.unwrap();
        assert!(local_deadline >= before_local + 60_000);

        // Account finishes last; its snapshot must carry that newer deadline.
        runtime.next_refresh_at_ms.store(1, Ordering::Release);
        let before_account = epoch_milliseconds();
        let schedule = runtime.reset_refresh_schedule();
        let account = runtime.cache_account_snapshot().unwrap();
        assert_eq!(account.revision, local.revision + 1);
        let account_deadline = account.refresh_schedule.unwrap().next_refresh_at_ms.unwrap();
        assert_eq!(Some(account_deadline), schedule.next_refresh_at_ms);
        assert!(account_deadline >= before_account + 60_000);
        let cached: serde_json::Value = serde_json::from_slice(
            &fs::read(&runtime.snapshot_cache_path).unwrap(),
        ).unwrap();
        assert_eq!(cached["refreshSchedule"]["nextRefreshAtMs"], account_deadline);
    }

    #[test]
    fn local_scan_and_pricing_save_do_not_wait_for_account_task() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        let pending = Arc::new(AtomicBool::new(false));
        *runtime.account_task.lock().unwrap() = Some(pending.clone());
        let snapshot = runtime.collect();
        assert!(snapshot.device_usage.is_some());
        assert_eq!(snapshot.status, "ready");
        assert!(!pending.load(Ordering::Acquire));
        let mut prices = PricingSettings::default();
        prices.peak.multiplier = 2.0;
        let saved = runtime.set_pricing_settings(&prices).unwrap();
        assert_eq!(saved.peak.multiplier, 2.0);
        assert_eq!(
            runtime.read_pricing_settings().unwrap().peak.multiplier,
            2.0
        );
        assert_eq!(runtime.collect().revision, snapshot.revision + 1);
    }

    #[test]
    fn local_only_persists_and_cancels_in_flight_account_work() {
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().join("data");
        let runtime = UsageRuntime::new(
            data.clone(),
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        let cancelled = Arc::new(AtomicBool::new(false));
        *runtime.account_task.lock().unwrap() = Some(cancelled.clone());
        let input: settings::AppPreferencesInput = serde_json::from_value(serde_json::json!({
            "localOnly": true, "refreshIntervalMinutes": 1, "closeBehavior": "hideToTray", "quotaWarningPercent": 20,
            "cacheWarningPercent": 90, "notificationsEnabled": false, "quietHoursEnabled": false, "quietHoursStart": "22:00", "quietHoursEnd": "08:00"
        })).unwrap();
        runtime.set_app_preferences(&input, false).unwrap();
        assert!(cancelled.load(Ordering::Acquire));
        assert!(runtime.account_task.lock().unwrap().is_none());
        let restarted = UsageRuntime::new(
            data,
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        assert!(restarted.local_only.load(Ordering::Acquire));
        let snapshot = restarted.collect();
        assert_eq!(snapshot.account_sync.unwrap().status, "localOnly");
        assert!(restarted.account_task.lock().unwrap().is_none());
    }

    #[test]
    fn legacy_dashboard_cache_does_not_revive_unverified_account_data() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        fs::write(&runtime.snapshot_cache_path, br#"{"revision":123,"quotaWindows":[{"private":"old"}],"accountUsage":{"old":true},"deviceUsage":{"sentinel":42}}"#).unwrap();
        let cache = runtime.read_cached_dashboard_snapshot().unwrap();
        assert_eq!(cache["revision"], 0);
        assert_eq!(cache["quotaWindows"], serde_json::json!([]));
        assert!(cache["accountUsage"].is_null());
        assert_eq!(cache["deviceUsage"]["sentinel"], 42);
        assert_eq!(cache["accountSync"]["quota"]["stale"], true);
    }

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
            local_only: false,
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
    fn maintenance_backup_contains_history_indexes_without_removing_originals() {
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
            .backup_index_databases()
            .expect("backup succeeds")
            .expect("backup timestamp");
        assert!(runtime.codex_database_path.exists());
        assert!(runtime.multi_source_database_path.exists());
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
    fn maintenance_backup_includes_committed_wal_pages() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let runtime = UsageRuntime::new(
            temporary.path().join("data"),
            temporary.path().join("codex"),
            temporary.path().join("home"),
        );
        fs::create_dir_all(&runtime.app_data_directory).unwrap();
        let writer = Connection::open(&runtime.codex_database_path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA wal_autocheckpoint=0;
                 CREATE TABLE latest_history(tokens INTEGER NOT NULL);
                 INSERT INTO latest_history VALUES(12345);",
            )
            .unwrap();
        assert!(
            family_member(&runtime.codex_database_path, "-wal")
                .metadata()
                .unwrap()
                .len()
                > 0
        );
        runtime.backup_index_databases().expect("consistent backup");
        let backup = fs::read_dir(runtime.app_data_directory.join("index-backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("usage-index-v1.sqlite3");
        let restored = open_read_only(&backup).unwrap();
        let tokens: i64 = restored
            .query_row("SELECT sum(tokens) FROM latest_history", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(tokens, 12345);
        assert!(quick_check_ok(&restored));
        assert_eq!(
            writer
                .query_row("SELECT sum(tokens) FROM latest_history", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            12345
        );
    }

    fn indexed_runtime_fixture(temporary: &Path) -> (UsageRuntime, PathBuf) {
        let codex_home = temporary.join("codex");
        let source = codex_home
            .join("sessions")
            .join("rollout-2026-08-28T00-00-00-00000000-0000-4000-8000-000000000001.jsonl");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        let lines = [
            serde_json::json!({"timestamp":"2026-08-28T00:00:00Z","type":"session_meta","payload":{"id":"00000000-0000-4000-8000-000000000001","cwd":"C:\\fixture","source":"cli"}}),
            serde_json::json!({"timestamp":"2026-08-28T00:00:00Z","type":"turn_context","payload":{"cwd":"C:\\fixture","model":"gpt-6-sol"}}),
            serde_json::json!({"timestamp":"2026-08-28T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":80,"output_tokens":5,"reasoning_output_tokens":0,"total_tokens":105}}}}),
        ];
        fs::write(
            &source,
            lines
                .iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let runtime = UsageRuntime::new(temporary.join("data"), codex_home, temporary.join("home"));
        let mut index = IncrementalIndex::open(&runtime.codex_database_path).unwrap();
        index.scan_file(&source).unwrap();
        assert_eq!(index.aggregate().unwrap().total.total_tokens, 105);
        (runtime, source)
    }

    #[test]
    fn rebuild_keeps_usage_when_original_source_is_missing() {
        let temporary = tempfile::tempdir().unwrap();
        let (runtime, source) = indexed_runtime_fixture(temporary.path());
        fs::remove_file(&source).unwrap();
        let rebuilt = runtime.rebuild_indexes_with_progress(|_| {}).unwrap();
        assert!(rebuilt.backup_created_at.is_some());
        let summary = rebuilt.snapshot.device_usage.unwrap();
        let codex = summary
            .sources
            .iter()
            .find(|source| source.id == "codex")
            .unwrap();
        assert_eq!(codex.total.total_tokens, 105);
        assert_eq!(codex.indexed_events, 1);
        assert_eq!(
            IncrementalIndex::open(&runtime.codex_database_path)
                .unwrap()
                .aggregate()
                .unwrap()
                .total
                .total_tokens,
            105
        );
    }

    #[test]
    fn rebuild_keeps_multi_source_usage_without_source_file() {
        let temporary = tempfile::tempdir().unwrap();
        let (runtime, _) = indexed_runtime_fixture(temporary.path());
        MultiSourceIndex::open(&runtime.multi_source_database_path).unwrap();
        let connection = Connection::open(&runtime.multi_source_database_path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO source_files(source, file_key, size_bytes, modified_at_ms, present)
             VALUES('claude-code', zeroblob(32), 100, 1, 1);
             INSERT INTO source_events(event_key, source, source_file_key, occurred_at_ms,
               model_label, project_label, input_tokens, cached_input_tokens, cache_write_tokens,
               output_tokens, reasoning_output_tokens, total_tokens)
             VALUES(zeroblob(32), 'claude-code', zeroblob(32), 1787875200000,
               'claude-sonnet-4', 'fixture', 40, 0, 0, 5, 0, 45);
             INSERT INTO source_event_files(source, source_file_key, event_key)
             VALUES('claude-code', zeroblob(32), zeroblob(32));",
            )
            .unwrap();
        drop(connection);
        let rebuilt = runtime.rebuild_indexes_with_progress(|_| {}).unwrap();
        let summary = rebuilt.snapshot.device_usage.unwrap();
        let claude = summary
            .sources
            .iter()
            .find(|source| source.id == "claude-code")
            .unwrap();
        assert_eq!(claude.total.total_tokens, 45);
        assert_eq!(claude.indexed_events, 1);
        let connection = open_read_only(&runtime.multi_source_database_path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT sum(total_tokens) FROM source_events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            45
        );
    }

    #[test]
    fn scan_failure_keeps_codex_history_visible_with_a_warning() {
        let temporary = tempfile::tempdir().unwrap();
        let (runtime, source) = indexed_runtime_fixture(temporary.path());
        fs::remove_file(&source).unwrap();
        fs::remove_dir(source.parent().unwrap()).unwrap();
        // A directory that has become a file reproduces a traversal failure
        // without platform-specific permission changes.
        fs::write(
            runtime.codex_home.join("sessions"),
            b"temporarily unavailable",
        )
        .unwrap();
        let snapshot = runtime.collect();
        let summary = snapshot.device_usage.unwrap();
        let codex = summary
            .sources
            .iter()
            .find(|source| source.id == "codex")
            .unwrap();
        assert_eq!(codex.total.total_tokens, 105);
        assert_eq!(codex.new_events, None);
        assert!(codex
            .message
            .as_deref()
            .is_some_and(|message| message.contains("未能完整读取")));
        assert!(summary
            .warnings
            .iter()
            .any(|warning| warning.contains("未能完整读取")));
    }

    #[test]
    fn failed_backup_aborts_rebuild_without_changing_original_index() {
        let temporary = tempfile::tempdir().unwrap();
        let (runtime, source) = indexed_runtime_fixture(temporary.path());
        let source_before = fs::read(&source).unwrap();
        let original = open_read_only(&runtime.codex_database_path).unwrap();
        let parser_before: i64 = original
            .query_row("SELECT parser_version FROM rollout_files", [], |row| {
                row.get(0)
            })
            .unwrap();
        drop(original);
        fs::write(
            runtime.app_data_directory.join("index-backups"),
            b"blocked destination",
        )
        .unwrap();
        assert!(runtime.rebuild_indexes_with_progress(|_| {}).is_err());
        let original = open_read_only(&runtime.codex_database_path).unwrap();
        assert!(quick_check_ok(&original));
        let parser_after: i64 = original
            .query_row("SELECT parser_version FROM rollout_files", [], |row| {
                row.get(0)
            })
            .unwrap();
        let tokens_after: i64 = original
            .query_row("SELECT sum(total_tokens) FROM usage_events", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(parser_after, parser_before);
        assert_eq!(tokens_after, 105);
        assert_eq!(fs::read(&source).unwrap(), source_before);
        assert_eq!(
            settings::read_last_index_backup_at(&runtime.settings_path).unwrap(),
            None
        );
    }

    #[test]
    fn second_database_backup_failure_leaves_both_originals_untouched() {
        let temporary = tempfile::tempdir().unwrap();
        let (runtime, source) = indexed_runtime_fixture(temporary.path());
        let corrupt = b"unreadable second database";
        fs::write(&runtime.multi_source_database_path, corrupt).unwrap();
        assert!(runtime.rebuild_indexes_with_progress(|_| {}).is_err());
        let original = open_read_only(&runtime.codex_database_path).unwrap();
        let (tokens, parser): (i64, i64) = original
            .query_row(
                "SELECT sum(e.total_tokens), min(f.parser_version)
                 FROM usage_events e JOIN rollout_files f ON f.id = e.file_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(tokens, 105);
        assert_eq!(parser, PARSER_SEMANTICS_VERSION);
        assert!(source.is_file());
        assert_eq!(
            fs::read(&runtime.multi_source_database_path).unwrap(),
            corrupt
        );
        assert_eq!(
            settings::read_last_index_backup_at(&runtime.settings_path).unwrap(),
            None
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
