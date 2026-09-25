use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use rfd::FileDialog;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::pricing::PricingSettings;

const PLAN_RENEWAL_KEY: &str = "plan-renewal-at";
const THEME_PREFERENCE_KEY: &str = "ui-theme-preference";
const BACKGROUND_FILE_KEY: &str = "ui-background-file";
const REFRESH_INTERVAL_KEY: &str = "refresh-interval-minutes";
const CLOSE_BEHAVIOR_KEY: &str = "close-behavior";
const QUOTA_WARNING_KEY: &str = "quota-warning-percent";
const CACHE_WARNING_KEY: &str = "cache-warning-percent";
const NOTIFICATIONS_ENABLED_KEY: &str = "notifications-enabled";
const LAST_NOTIFICATION_AT_KEY: &str = "last-notification-at";
const LAST_NOTIFICATION_REASON_KEY: &str = "last-notification-reason";
const LAST_INDEX_BACKUP_AT_KEY: &str = "last-index-backup-at";
const QUIET_HOURS_ENABLED_KEY: &str = "quiet-hours-enabled";
const QUIET_HOURS_START_KEY: &str = "quiet-hours-start";
const QUIET_HOURS_END_KEY: &str = "quiet-hours-end";
const PROJECT_MERGE_RULES_KEY: &str = "project-merge-rules-v1";
const PRICING_SETTINGS_KEY: &str = "model-pricing-v1";
const BACKGROUND_DIRECTORY: &str = "backgrounds";
const MAX_BACKGROUND_BYTES: u64 = 20 * 1024 * 1024;
const MAX_PROJECT_MERGE_RULES: usize = 50;
const MAX_PROJECT_MERGE_MEMBERS: usize = 100;
const MAX_PROJECT_FIELD_LENGTH: usize = 256;
const MAX_PRICING_MODELS: usize = 200;

#[derive(Debug)]
pub(crate) enum SettingsError {
    InvalidDate,
    InvalidPreference,
    InvalidProjectMergeRules,
    InvalidPricingSettings,
    Storage,
    InvalidBackground(String),
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ThemePreference {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VisualPreferences {
    pub(crate) theme_preference: ThemePreference,
    pub(crate) background_asset_path: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CloseBehavior {
    HideToTray,
    Exit,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppPreferencesInput {
    pub(crate) refresh_interval_minutes: u64,
    pub(crate) close_behavior: CloseBehavior,
    pub(crate) quota_warning_percent: u8,
    pub(crate) cache_warning_percent: u8,
    pub(crate) notifications_enabled: bool,
    pub(crate) quiet_hours_enabled: bool,
    pub(crate) quiet_hours_start: String,
    pub(crate) quiet_hours_end: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppPreferences {
    pub(crate) theme_preference: ThemePreference,
    pub(crate) background_asset_path: Option<String>,
    pub(crate) refresh_interval_minutes: u64,
    pub(crate) close_behavior: CloseBehavior,
    pub(crate) autostart_enabled: bool,
    pub(crate) quota_warning_percent: u8,
    pub(crate) cache_warning_percent: u8,
    pub(crate) notifications_enabled: bool,
    pub(crate) last_notification_at: Option<String>,
    pub(crate) last_notification_reason: Option<String>,
    pub(crate) quiet_hours_enabled: bool,
    pub(crate) quiet_hours_start: String,
    pub(crate) quiet_hours_end: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectMergeMember {
    pub(crate) source_id: String,
    pub(crate) source_label: String,
    pub(crate) project_key: String,
    pub(crate) project_label: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectMergeRule {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) members: Vec<ProjectMergeMember>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct ProjectMergeRulesDocument {
    version: u8,
    rules: Vec<ProjectMergeRule>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
struct PricingSettingsDocument {
    version: u8,
    settings: PricingSettings,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanRenewalSetting {
    pub(crate) plan_renewal_at: Option<String>,
    pub(crate) plan_renewal_source: Option<&'static str>,
}

pub(crate) fn normalize_plan_renewal_at(value: &str) -> Result<String, SettingsError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| SettingsError::InvalidDate)?;
    if !(2000..=2100).contains(&parsed.year()) {
        return Err(SettingsError::InvalidDate);
    }
    Ok(parsed
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Secs, true))
}

pub(crate) fn read_plan_renewal_at(database_path: &Path) -> Result<Option<String>, SettingsError> {
    let connection = open(database_path)?;
    connection
        .query_row(
            "SELECT value FROM app_settings WHERE key = ?1",
            [PLAN_RENEWAL_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| SettingsError::Storage)
}

pub(crate) fn write_plan_renewal_at(
    database_path: &Path,
    value: Option<&str>,
) -> Result<PlanRenewalSetting, SettingsError> {
    let mut connection = open(database_path)?;
    let transaction = connection
        .transaction()
        .map_err(|_| SettingsError::Storage)?;
    let normalized = match value {
        Some(value) => {
            let normalized = normalize_plan_renewal_at(value)?;
            transaction
                .execute(
                    "INSERT INTO app_settings(key, value) VALUES (?1, ?2)\
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![PLAN_RENEWAL_KEY, normalized],
                )
                .map_err(|_| SettingsError::Storage)?;
            Some(normalized)
        }
        None => {
            transaction
                .execute(
                    "DELETE FROM app_settings WHERE key = ?1",
                    [PLAN_RENEWAL_KEY],
                )
                .map_err(|_| SettingsError::Storage)?;
            None
        }
    };
    transaction.commit().map_err(|_| SettingsError::Storage)?;
    Ok(PlanRenewalSetting {
        plan_renewal_source: normalized.as_ref().map(|_| "manual"),
        plan_renewal_at: normalized,
    })
}

fn open(database_path: &Path) -> Result<Connection, SettingsError> {
    if let Some(parent) = database_path.parent() {
        fs::create_dir_all(parent).map_err(|_| SettingsError::Storage)?;
    }
    let connection = Connection::open(database_path).map_err(|_| SettingsError::Storage)?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| SettingsError::Storage)?;
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS app_settings (
               key TEXT PRIMARY KEY NOT NULL,
               value TEXT NOT NULL
             );",
        )
        .map_err(|_| SettingsError::Storage)?;
    Ok(connection)
}

fn read_setting(connection: &Connection, key: &str) -> Result<Option<String>, SettingsError> {
    connection
        .query_row(
            "SELECT value FROM app_settings WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| SettingsError::Storage)
}

fn write_setting(connection: &Connection, key: &str, value: &str) -> Result<(), SettingsError> {
    connection
        .execute(
            "INSERT INTO app_settings(key, value) VALUES (?1, ?2)\
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(|_| SettingsError::Storage)?;
    Ok(())
}

pub(crate) fn validate_project_merge_rules(
    rules: &[ProjectMergeRule],
) -> Result<(), SettingsError> {
    if rules.len() > MAX_PROJECT_MERGE_RULES {
        return Err(SettingsError::InvalidProjectMergeRules);
    }
    let allowed_sources = [
        "codex",
        "claude-code",
        "opencode",
        "workbuddy",
        "workbuddy-ai",
        "cursor",
    ];
    let mut rule_ids = HashSet::new();
    let mut assigned_members = HashSet::new();
    for rule in rules {
        let valid_field = |value: &str| {
            let length = value.chars().count();
            length > 0 && length <= MAX_PROJECT_FIELD_LENGTH && value.trim() == value
        };
        if !valid_field(&rule.id)
            || !valid_field(&rule.display_name)
            || !rule_ids.insert(rule.id.as_str())
            || rule.members.len() < 2
            || rule.members.len() > MAX_PROJECT_MERGE_MEMBERS
            || !rule
                .members
                .iter()
                .any(|member| member.project_label == rule.display_name)
        {
            return Err(SettingsError::InvalidProjectMergeRules);
        }
        let mut local_members = HashSet::new();
        for member in &rule.members {
            if !allowed_sources.contains(&member.source_id.as_str())
                || !valid_field(&member.source_label)
                || !valid_field(&member.project_key)
                || !valid_field(&member.project_label)
            {
                return Err(SettingsError::InvalidProjectMergeRules);
            }
            let identity = format!("{}\u{0}{}", member.source_id, member.project_key);
            if !local_members.insert(identity.clone()) || !assigned_members.insert(identity) {
                return Err(SettingsError::InvalidProjectMergeRules);
            }
        }
    }
    Ok(())
}

pub(crate) fn read_project_merge_rules(
    database_path: &Path,
) -> Result<Vec<ProjectMergeRule>, SettingsError> {
    let connection = open(database_path)?;
    let Some(value) = read_setting(&connection, PROJECT_MERGE_RULES_KEY)? else {
        return Ok(Vec::new());
    };
    let document: ProjectMergeRulesDocument =
        serde_json::from_str(&value).map_err(|_| SettingsError::InvalidProjectMergeRules)?;
    if document.version != 1 {
        return Err(SettingsError::InvalidProjectMergeRules);
    }
    validate_project_merge_rules(&document.rules)?;
    Ok(document.rules)
}

pub(crate) fn write_project_merge_rules(
    database_path: &Path,
    rules: &[ProjectMergeRule],
) -> Result<Vec<ProjectMergeRule>, SettingsError> {
    validate_project_merge_rules(rules)?;
    let value = serde_json::to_string(&ProjectMergeRulesDocument {
        version: 1,
        rules: rules.to_vec(),
    })
    .map_err(|_| SettingsError::InvalidProjectMergeRules)?;
    let mut connection = open(database_path)?;
    let transaction = connection
        .transaction()
        .map_err(|_| SettingsError::Storage)?;
    write_setting(&transaction, PROJECT_MERGE_RULES_KEY, &value)?;
    transaction.commit().map_err(|_| SettingsError::Storage)?;
    Ok(rules.to_vec())
}

pub(crate) fn validate_pricing_settings(settings: &PricingSettings) -> Result<(), SettingsError> {
    if !valid_clock(&settings.peak.start_time)
        || !valid_clock(&settings.peak.end_time)
        || !settings.peak.multiplier.is_finite()
        || !(1.0..=100.0).contains(&settings.peak.multiplier)
        || settings.models.len() > MAX_PRICING_MODELS
    {
        return Err(SettingsError::InvalidPricingSettings);
    }
    let mut identities = HashSet::new();
    for rule in &settings.models {
        let valid_text = |value: &str| {
            let length = value.chars().count();
            value.trim() == value && !value.is_empty() && length <= MAX_PROJECT_FIELD_LENGTH
        };
        let valid_rate = |rate: Option<f64>| {
            rate.map_or(true, |value| {
                value.is_finite() && (0.0..=1_000_000.0).contains(&value)
            })
        };
        if !valid_text(&rule.model_id)
            || !valid_text(&rule.display_name)
            || !matches!(rule.currency.as_str(), "USD" | "CNY")
            || !valid_rate(rule.input_per_million)
            || !valid_rate(rule.cached_input_per_million)
            || !valid_rate(rule.cache_write_per_million)
            || !valid_rate(rule.output_per_million)
            || !identities.insert(rule.model_id.to_lowercase())
        {
            return Err(SettingsError::InvalidPricingSettings);
        }
    }
    Ok(())
}

pub(crate) fn read_pricing_settings(
    database_path: &Path,
) -> Result<PricingSettings, SettingsError> {
    let connection = open(database_path)?;
    let Some(value) = read_setting(&connection, PRICING_SETTINGS_KEY)? else {
        return Ok(PricingSettings::default());
    };
    let document: PricingSettingsDocument =
        serde_json::from_str(&value).map_err(|_| SettingsError::InvalidPricingSettings)?;
    if document.version != 1 {
        return Err(SettingsError::InvalidPricingSettings);
    }
    validate_pricing_settings(&document.settings)?;
    Ok(document.settings)
}

pub(crate) fn write_pricing_settings(
    database_path: &Path,
    settings: &PricingSettings,
) -> Result<PricingSettings, SettingsError> {
    validate_pricing_settings(settings)?;
    let mut saved = settings.clone();
    saved.updated_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true));
    let value = serde_json::to_string(&PricingSettingsDocument {
        version: 1,
        settings: saved.clone(),
    })
    .map_err(|_| SettingsError::InvalidPricingSettings)?;
    let mut connection = open(database_path)?;
    let transaction = connection
        .transaction()
        .map_err(|_| SettingsError::Storage)?;
    write_setting(&transaction, PRICING_SETTINGS_KEY, &value)?;
    transaction.commit().map_err(|_| SettingsError::Storage)?;
    Ok(saved)
}

fn parse_theme_preference(value: Option<String>) -> ThemePreference {
    match value.as_deref() {
        Some("light") => ThemePreference::Light,
        Some("dark") => ThemePreference::Dark,
        _ => ThemePreference::System,
    }
}

fn bool_setting(value: Option<String>) -> bool {
    value.as_deref() == Some("true")
}

fn valid_clock(value: &str) -> bool {
    chrono::NaiveTime::parse_from_str(value, "%H:%M").is_ok()
}

fn read_u64_setting(
    connection: &Connection,
    key: &str,
    fallback: u64,
) -> Result<u64, SettingsError> {
    Ok(read_setting(connection, key)?
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(fallback))
}

pub(crate) fn read_app_preferences(
    settings_path: &Path,
    app_data_dir: &Path,
    autostart_enabled: bool,
) -> Result<AppPreferences, SettingsError> {
    let visual = read_visual_preferences(settings_path, app_data_dir)?;
    let connection = open(settings_path)?;
    let stored_refresh_interval = read_u64_setting(&connection, REFRESH_INTERVAL_KEY, 1)?;
    let refresh_interval_minutes = match stored_refresh_interval {
        1 | 5 | 15 => stored_refresh_interval,
        _ => 1,
    };
    let close_behavior = match read_setting(&connection, CLOSE_BEHAVIOR_KEY)?.as_deref() {
        Some("exit") => CloseBehavior::Exit,
        _ => CloseBehavior::HideToTray,
    };
    let quota_warning_percent =
        read_u64_setting(&connection, QUOTA_WARNING_KEY, 20)?.clamp(1, 99) as u8;
    let cache_warning_percent =
        read_u64_setting(&connection, CACHE_WARNING_KEY, 90)?.clamp(1, 100) as u8;
    let quiet_hours_start = read_setting(&connection, QUIET_HOURS_START_KEY)?
        .filter(|value| valid_clock(value))
        .unwrap_or_else(|| "22:00".to_owned());
    let quiet_hours_end = read_setting(&connection, QUIET_HOURS_END_KEY)?
        .filter(|value| valid_clock(value))
        .unwrap_or_else(|| "08:00".to_owned());
    Ok(AppPreferences {
        theme_preference: visual.theme_preference,
        background_asset_path: visual.background_asset_path,
        refresh_interval_minutes,
        close_behavior,
        autostart_enabled,
        quota_warning_percent,
        cache_warning_percent,
        notifications_enabled: read_setting(&connection, NOTIFICATIONS_ENABLED_KEY)?.as_deref()
            != Some("false"),
        last_notification_at: read_setting(&connection, LAST_NOTIFICATION_AT_KEY)?,
        last_notification_reason: read_setting(&connection, LAST_NOTIFICATION_REASON_KEY)?,
        quiet_hours_enabled: bool_setting(read_setting(&connection, QUIET_HOURS_ENABLED_KEY)?),
        quiet_hours_start,
        quiet_hours_end,
    })
}

pub(crate) fn write_app_preferences(
    settings_path: &Path,
    app_data_dir: &Path,
    input: &AppPreferencesInput,
    autostart_enabled: bool,
) -> Result<AppPreferences, SettingsError> {
    validate_app_preferences(input)?;
    let mut connection = open(settings_path)?;

    let transaction = connection
        .transaction()
        .map_err(|_| SettingsError::Storage)?;
    write_setting(
        &transaction,
        REFRESH_INTERVAL_KEY,
        &input.refresh_interval_minutes.to_string(),
    )?;
    write_setting(
        &transaction,
        CLOSE_BEHAVIOR_KEY,
        match input.close_behavior {
            CloseBehavior::HideToTray => "hideToTray",
            CloseBehavior::Exit => "exit",
        },
    )?;
    write_setting(
        &transaction,
        QUOTA_WARNING_KEY,
        &input.quota_warning_percent.to_string(),
    )?;
    write_setting(
        &transaction,
        CACHE_WARNING_KEY,
        &input.cache_warning_percent.to_string(),
    )?;
    write_setting(
        &transaction,
        NOTIFICATIONS_ENABLED_KEY,
        if input.notifications_enabled {
            "true"
        } else {
            "false"
        },
    )?;
    write_setting(
        &transaction,
        QUIET_HOURS_ENABLED_KEY,
        if input.quiet_hours_enabled {
            "true"
        } else {
            "false"
        },
    )?;
    write_setting(
        &transaction,
        QUIET_HOURS_START_KEY,
        &input.quiet_hours_start,
    )?;
    write_setting(&transaction, QUIET_HOURS_END_KEY, &input.quiet_hours_end)?;
    transaction.commit().map_err(|_| SettingsError::Storage)?;
    read_app_preferences(settings_path, app_data_dir, autostart_enabled)
}

pub(crate) fn validate_app_preferences(input: &AppPreferencesInput) -> Result<(), SettingsError> {
    if !matches!(input.refresh_interval_minutes, 1 | 5 | 15)
        || !(1..=99).contains(&input.quota_warning_percent)
        || !(1..=100).contains(&input.cache_warning_percent)
        || !valid_clock(&input.quiet_hours_start)
        || !valid_clock(&input.quiet_hours_end)
    {
        return Err(SettingsError::InvalidPreference);
    }
    Ok(())
}

pub(crate) fn record_notification(settings_path: &Path, reason: &str) -> Result<(), SettingsError> {
    if reason.trim().is_empty() || reason.chars().count() > 160 {
        return Err(SettingsError::InvalidPreference);
    }
    let mut connection = open(settings_path)?;
    let transaction = connection
        .transaction()
        .map_err(|_| SettingsError::Storage)?;
    write_setting(
        &transaction,
        LAST_NOTIFICATION_AT_KEY,
        &Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
    )?;
    write_setting(&transaction, LAST_NOTIFICATION_REASON_KEY, reason)?;
    transaction.commit().map_err(|_| SettingsError::Storage)
}

pub(crate) fn read_last_index_backup_at(
    settings_path: &Path,
) -> Result<Option<String>, SettingsError> {
    let connection = open(settings_path)?;
    read_setting(&connection, LAST_INDEX_BACKUP_AT_KEY)
}

pub(crate) fn record_index_backup(
    settings_path: &Path,
    created_at: &str,
) -> Result<(), SettingsError> {
    let normalized = normalize_plan_renewal_at(created_at)?;
    let connection = open(settings_path)?;
    write_setting(&connection, LAST_INDEX_BACKUP_AT_KEY, &normalized)
}

fn is_background_file_name(value: &str) -> bool {
    let path = Path::new(value);
    let extension = path.extension().and_then(|extension| extension.to_str());
    path.parent() == Some(Path::new(""))
        && value.starts_with("background-")
        && matches!(extension, Some("png" | "jpg" | "webp"))
}

fn background_path(app_data_dir: &Path, file_name: &str) -> Option<PathBuf> {
    is_background_file_name(file_name)
        .then(|| app_data_dir.join(BACKGROUND_DIRECTORY).join(file_name))
}

fn background_extension(source: &Path) -> Result<&'static str, SettingsError> {
    let mut file = fs::File::open(source)
        .map_err(|error| SettingsError::InvalidBackground(format!("无法读取所选文件：{error}")))?;
    let mut header = [0_u8; 12];
    let length = file
        .read(&mut header)
        .map_err(|error| SettingsError::InvalidBackground(format!("无法读取所选文件：{error}")))?;
    let header = &header[..length];

    if header.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Ok("png");
    }
    if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok("jpg");
    }
    if header.len() >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"WEBP" {
        return Ok("webp");
    }

    Err(SettingsError::InvalidBackground(
        "仅支持 PNG、JPEG 或 WebP 图片".to_owned(),
    ))
}

pub(crate) fn read_visual_preferences(
    settings_path: &Path,
    app_data_dir: &Path,
) -> Result<VisualPreferences, SettingsError> {
    let connection = open(settings_path)?;
    let theme_preference = parse_theme_preference(read_setting(&connection, THEME_PREFERENCE_KEY)?);
    let background_asset_path = read_setting(&connection, BACKGROUND_FILE_KEY)?
        .as_deref()
        .and_then(|file_name| background_path(app_data_dir, file_name))
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned());

    Ok(VisualPreferences {
        theme_preference,
        background_asset_path,
    })
}

pub(crate) fn set_theme_preference(
    settings_path: &Path,
    app_data_dir: &Path,
    theme_preference: ThemePreference,
) -> Result<VisualPreferences, SettingsError> {
    let connection = open(settings_path)?;
    let value = match theme_preference {
        ThemePreference::System => "system",
        ThemePreference::Light => "light",
        ThemePreference::Dark => "dark",
    };
    write_setting(&connection, THEME_PREFERENCE_KEY, value)?;
    drop(connection);
    read_visual_preferences(settings_path, app_data_dir)
}

pub(crate) fn select_background_image(
    settings_path: &Path,
    app_data_dir: &Path,
) -> Result<VisualPreferences, SettingsError> {
    let Some(source) = FileDialog::new()
        .add_filter("图片", &["png", "jpg", "jpeg", "webp"])
        .pick_file()
    else {
        return read_visual_preferences(settings_path, app_data_dir);
    };

    save_background_from_path(settings_path, app_data_dir, &source)
}

pub(crate) fn save_background_from_path(
    settings_path: &Path,
    app_data_dir: &Path,
    source: &Path,
) -> Result<VisualPreferences, SettingsError> {
    let metadata = fs::metadata(source)
        .map_err(|error| SettingsError::InvalidBackground(format!("无法读取所选文件：{error}")))?;
    if !metadata.is_file() {
        return Err(SettingsError::InvalidBackground(
            "所选路径不是文件".to_owned(),
        ));
    }
    if metadata.len() > MAX_BACKGROUND_BYTES {
        return Err(SettingsError::InvalidBackground(
            "图片不能超过 20 MB".to_owned(),
        ));
    }

    let extension = background_extension(source)?;
    let directory = app_data_dir.join(BACKGROUND_DIRECTORY);
    fs::create_dir_all(&directory)
        .map_err(|error| SettingsError::InvalidBackground(format!("无法创建背景目录：{error}")))?;
    let file_name = format!(
        "background-{}-{}.{}",
        Utc::now().timestamp_millis(),
        std::process::id(),
        extension
    );
    let destination = directory.join(&file_name);
    let copied = fs::copy(source, &destination)
        .map_err(|error| SettingsError::InvalidBackground(format!("无法复制背景图片：{error}")))?;
    if copied != metadata.len() {
        let _ = fs::remove_file(&destination);
        return Err(SettingsError::InvalidBackground(
            "背景图片复制不完整".to_owned(),
        ));
    }

    let connection = open(settings_path)?;
    let previous = read_setting(&connection, BACKGROUND_FILE_KEY)?;
    if let Err(error) = write_setting(&connection, BACKGROUND_FILE_KEY, &file_name) {
        let _ = fs::remove_file(&destination);
        return Err(error);
    }
    drop(connection);

    if let Some(path) = previous
        .as_deref()
        .and_then(|file_name| background_path(app_data_dir, file_name))
    {
        let _ = fs::remove_file(path);
    }
    read_visual_preferences(settings_path, app_data_dir)
}

pub(crate) fn clear_background_image(
    settings_path: &Path,
    app_data_dir: &Path,
) -> Result<VisualPreferences, SettingsError> {
    let connection = open(settings_path)?;
    let previous = read_setting(&connection, BACKGROUND_FILE_KEY)?;
    connection
        .execute(
            "DELETE FROM app_settings WHERE key = ?1",
            [BACKGROUND_FILE_KEY],
        )
        .map_err(|_| SettingsError::Storage)?;
    drop(connection);

    if let Some(path) = previous
        .as_deref()
        .and_then(|file_name| background_path(app_data_dir, file_name))
    {
        let _ = fs::remove_file(path);
    }
    read_visual_preferences(settings_path, app_data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_normalized_manual_renewal_and_can_clear_it() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let path = temporary.path().join("settings.sqlite3");
        let saved = write_plan_renewal_at(&path, Some("2027-02-03T12:34:56+08:00")).expect("write");
        assert_eq!(
            saved.plan_renewal_at.as_deref(),
            Some("2027-02-03T04:34:56Z")
        );
        assert_eq!(saved.plan_renewal_source, Some("manual"));
        assert_eq!(
            read_plan_renewal_at(&path).expect("read"),
            Some("2027-02-03T04:34:56Z".to_owned())
        );
        let cleared = write_plan_renewal_at(&path, None).expect("clear");
        assert_eq!(cleared.plan_renewal_source, None);
        assert_eq!(read_plan_renewal_at(&path).expect("read"), None);
    }

    #[test]
    fn rejects_invalid_or_unreasonable_dates() {
        assert!(matches!(
            normalize_plan_renewal_at("not-a-date"),
            Err(SettingsError::InvalidDate)
        ));
        assert!(matches!(
            normalize_plan_renewal_at("2200-01-01T00:00:00Z"),
            Err(SettingsError::InvalidDate)
        ));
    }

    fn merge_member(source_id: &str, source_label: &str, project: &str) -> ProjectMergeMember {
        ProjectMergeMember {
            source_id: source_id.to_owned(),
            source_label: source_label.to_owned(),
            project_key: project.to_owned(),
            project_label: project.to_owned(),
        }
    }

    #[test]
    fn persists_versioned_project_merge_rules() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let path = temporary.path().join("settings.sqlite3");
        let rules = vec![ProjectMergeRule {
            id: "work-log-history".to_owned(),
            display_name: "L.Q记工本".to_owned(),
            members: vec![
                merge_member("codex", "Codex", "L.Q记工本"),
                merge_member("claude-code", "Claude Code", "旧记工本"),
            ],
        }];
        assert_eq!(
            write_project_merge_rules(&path, &rules).expect("write rules"),
            rules
        );
        assert_eq!(read_project_merge_rules(&path).expect("read rules"), rules);
        let connection = open(&path).expect("open");
        let stored = read_setting(&connection, PROJECT_MERGE_RULES_KEY)
            .expect("read setting")
            .expect("stored value");
        assert!(stored.contains("\"version\":1"));
    }

    #[test]
    fn rejects_conflicting_project_merge_rules_without_overwriting_the_last_good_value() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let path = temporary.path().join("settings.sqlite3");
        let members = vec![
            merge_member("codex", "Codex", "L.Q记工本"),
            merge_member("claude-code", "Claude Code", "旧记工本"),
        ];
        let good = vec![ProjectMergeRule {
            id: "good".to_owned(),
            display_name: "L.Q记工本".to_owned(),
            members: members.clone(),
        }];
        write_project_merge_rules(&path, &good).expect("write good rules");
        let invalid = vec![
            good[0].clone(),
            ProjectMergeRule {
                id: "conflict".to_owned(),
                display_name: "旧记工本".to_owned(),
                members,
            },
        ];
        assert!(matches!(
            write_project_merge_rules(&path, &invalid),
            Err(SettingsError::InvalidProjectMergeRules)
        ));
        assert_eq!(
            read_project_merge_rules(&path).expect("read good rules"),
            good
        );
    }

    #[test]
    fn keeps_an_application_owned_background_after_the_source_is_removed() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let settings_path = temporary.path().join("settings.sqlite3");
        let app_data_dir = temporary.path().join("app-data");
        let source = temporary.path().join("source.png");
        fs::write(
            &source,
            [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0],
        )
        .expect("fixture image");

        let saved = save_background_from_path(&settings_path, &app_data_dir, &source)
            .expect("save background");
        let copied = PathBuf::from(saved.background_asset_path.expect("asset path"));
        assert!(copied.is_file());
        fs::remove_file(&source).expect("remove source");
        assert!(read_visual_preferences(&settings_path, &app_data_dir)
            .expect("read preferences")
            .background_asset_path
            .as_deref()
            .is_some_and(|path| Path::new(path).is_file()));

        let themed = set_theme_preference(&settings_path, &app_data_dir, ThemePreference::Light)
            .expect("persist theme");
        assert_eq!(themed.theme_preference, ThemePreference::Light);
        clear_background_image(&settings_path, &app_data_dir).expect("clear background");
        assert!(!copied.exists());
    }

    #[test]
    fn rejects_an_unrecognised_background_format() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let source = temporary.path().join("not-an-image.txt");
        fs::write(&source, b"plain text").expect("fixture");
        assert!(matches!(
            save_background_from_path(
                &temporary.path().join("settings.sqlite3"),
                &temporary.path().join("app-data"),
                &source
            ),
            Err(SettingsError::InvalidBackground(_))
        ));
    }

    #[test]
    fn persists_runtime_preferences_and_rejects_unsafe_values() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let settings_path = temporary.path().join("settings.sqlite3");
        let input = AppPreferencesInput {
            refresh_interval_minutes: 5,
            close_behavior: CloseBehavior::Exit,
            quota_warning_percent: 15,
            cache_warning_percent: 88,
            notifications_enabled: true,
            quiet_hours_enabled: true,
            quiet_hours_start: "23:30".to_owned(),
            quiet_hours_end: "07:15".to_owned(),
        };
        let saved = write_app_preferences(&settings_path, temporary.path(), &input, true)
            .expect("write preferences");
        assert_eq!(saved.refresh_interval_minutes, 5);
        assert_eq!(saved.close_behavior, CloseBehavior::Exit);
        assert!(saved.autostart_enabled);
        assert!(saved.notifications_enabled);
        assert_eq!(saved.last_notification_at, None);
        record_notification(&settings_path, "短周期额度剩余 12%").expect("record notification");
        let notified = read_app_preferences(&settings_path, temporary.path(), true)
            .expect("read notification status");
        assert!(notified.last_notification_at.is_some());
        assert_eq!(
            notified.last_notification_reason.as_deref(),
            Some("短周期额度剩余 12%")
        );
        assert_eq!(
            notified.refresh_interval_minutes,
            saved.refresh_interval_minutes
        );

        let invalid = AppPreferencesInput {
            refresh_interval_minutes: 2,
            ..input
        };
        assert!(matches!(
            write_app_preferences(&settings_path, temporary.path(), &invalid, false),
            Err(SettingsError::InvalidPreference)
        ));
    }

    #[test]
    fn persists_manual_pricing_without_compiled_model_defaults() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let path = temporary.path().join("settings.sqlite3");
        assert!(read_pricing_settings(&path)
            .expect("read defaults")
            .models
            .is_empty());
        let settings = PricingSettings {
            updated_at: None,
            peak: crate::pricing::PeakPricingSchedule {
                start_time: "18:30".to_owned(),
                end_time: "22:15".to_owned(),
                multiplier: 1.75,
            },
            models: vec![crate::pricing::ModelPricingRule {
                model_id: "my-model".to_owned(),
                display_name: "My Model".to_owned(),
                currency: "USD".to_owned(),
                input_per_million: Some(2.0),
                cached_input_per_million: Some(0.2),
                cache_write_per_million: None,
                output_per_million: Some(8.0),
                peak_enabled: true,
            }],
        };
        let saved = write_pricing_settings(&path, &settings).expect("save pricing");
        assert!(saved.updated_at.is_some());
        assert_eq!(read_pricing_settings(&path).expect("read pricing"), saved);
    }

    #[test]
    fn rejects_invalid_pricing_without_overwriting_last_good_value() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let path = temporary.path().join("settings.sqlite3");
        let good = PricingSettings::default();
        write_pricing_settings(&path, &good).expect("save defaults");
        let invalid = PricingSettings {
            peak: crate::pricing::PeakPricingSchedule {
                multiplier: 0.5,
                ..good.peak.clone()
            },
            ..good.clone()
        };
        assert!(matches!(
            write_pricing_settings(&path, &invalid),
            Err(SettingsError::InvalidPricingSettings)
        ));
        assert_eq!(
            read_pricing_settings(&path).expect("last good").peak,
            good.peak
        );
    }
}
