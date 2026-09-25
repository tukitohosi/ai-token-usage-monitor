mod app_server;
pub mod index;
mod multisource;
mod pricing;
mod runtime;
mod settings;
mod snapshot;
mod tray;

use std::path::PathBuf;

use tauri::{Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;

#[tauri::command]
async fn read_dashboard_snapshot(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, runtime::UsageRuntime>,
) -> Result<snapshot::DashboardSnapshot, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let snapshot = runtime.collect_with_progress(|progress| {
            let _ = app.emit(runtime::PROGRESS_EVENT, progress);
        });
        runtime::update_tray_tooltip(&app, &snapshot);
        runtime.maybe_notify(&app, &snapshot);
        snapshot
    })
    .await
    .map_err(|_| "后台用量读取任务意外结束。".to_owned())
}

#[tauri::command]
async fn read_cached_dashboard_snapshot(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
) -> Result<Option<serde_json::Value>, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.read_cached_dashboard_snapshot())
        .await
        .map_err(|_| "后台快照缓存读取任务意外结束。".to_owned())
}

#[tauri::command(rename_all = "camelCase")]
async fn set_plan_renewal_at(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
    plan_renewal_at: Option<String>,
) -> Result<settings::PlanRenewalSetting, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        runtime.set_plan_renewal_at(plan_renewal_at.as_deref())
    })
    .await
    .map_err(|_| "后台设置任务意外结束。".to_owned())?
    .map_err(|error| match error {
        settings::SettingsError::InvalidDate => "请输入有效的续费日期。".to_owned(),
        settings::SettingsError::InvalidPreference => "本机续费时间设置无效。".to_owned(),
        settings::SettingsError::InvalidProjectMergeRules => "项目合并规则无效。".to_owned(),
        settings::SettingsError::InvalidPricingSettings => "模型定价设置无效。".to_owned(),
        settings::SettingsError::Storage => "无法保存本机续费时间设置。".to_owned(),
        settings::SettingsError::InvalidBackground(_) => "无法保存本机续费时间设置。".to_owned(),
    })
}

#[tauri::command]
async fn read_visual_preferences(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
) -> Result<settings::VisualPreferences, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.read_visual_preferences())
        .await
        .map_err(|_| "后台视觉设置读取任务意外结束。".to_owned())?
        .map_err(map_visual_settings_error)
}

#[tauri::command(rename_all = "camelCase")]
async fn set_theme_preference(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
    theme_preference: settings::ThemePreference,
) -> Result<settings::VisualPreferences, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.set_theme_preference(theme_preference))
        .await
        .map_err(|_| "后台视觉设置保存任务意外结束。".to_owned())?
        .map_err(map_visual_settings_error)
}

#[tauri::command]
async fn select_background_image(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
) -> Result<settings::VisualPreferences, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.select_background_image())
        .await
        .map_err(|_| "后台背景图片选择任务意外结束。".to_owned())?
        .map_err(map_visual_settings_error)
}

#[tauri::command]
async fn clear_background_image(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
) -> Result<settings::VisualPreferences, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.clear_background_image())
        .await
        .map_err(|_| "后台背景图片清理任务意外结束。".to_owned())?
        .map_err(map_visual_settings_error)
}

fn map_visual_settings_error(error: settings::SettingsError) -> String {
    match error {
        settings::SettingsError::InvalidBackground(message) => message,
        settings::SettingsError::InvalidDate
        | settings::SettingsError::InvalidPreference
        | settings::SettingsError::InvalidProjectMergeRules
        | settings::SettingsError::InvalidPricingSettings
        | settings::SettingsError::Storage => "无法保存本机视觉设置。".to_owned(),
    }
}

#[tauri::command]
async fn read_app_preferences(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, runtime::UsageRuntime>,
) -> Result<settings::AppPreferences, String> {
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.read_app_preferences(autostart_enabled))
        .await
        .map_err(|_| "后台应用设置读取任务意外结束。".to_owned())?
        .map_err(|_| "无法读取本机应用设置。".to_owned())
}

#[tauri::command(rename_all = "camelCase")]
async fn set_app_preferences(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, runtime::UsageRuntime>,
    preferences: settings::AppPreferencesInput,
    autostart_enabled: bool,
) -> Result<settings::AppPreferences, String> {
    settings::validate_app_preferences(&preferences)
        .map_err(|_| "应用设置参数无效。".to_owned())?;
    let result = if autostart_enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    };
    let effective_autostart = match result {
        Ok(()) => autostart_enabled,
        Err(_) => app.autolaunch().is_enabled().unwrap_or(!autostart_enabled),
    };
    if let Some(handle) = app.try_state::<tray::TrayMenuHandle<tauri::Wry>>() {
        let _ = handle.set_autostart_enabled(effective_autostart);
    }
    let runtime = runtime.inner().clone();
    let settings_runtime = runtime.clone();
    let saved = tauri::async_runtime::spawn_blocking(move || {
        settings_runtime.set_app_preferences(&preferences, effective_autostart)
    })
    .await
    .map_err(|_| "后台应用设置保存任务意外结束。".to_owned())?
    .map_err(|error| match error {
        settings::SettingsError::InvalidPreference => "应用设置参数无效。".to_owned(),
        _ => "无法保存本机应用设置。".to_owned(),
    })?;
    let _ = app.emit(runtime::REFRESH_SCHEDULE_EVENT, runtime.refresh_schedule());
    Ok(saved)
}

#[tauri::command]
async fn read_project_merge_rules(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
) -> Result<Vec<settings::ProjectMergeRule>, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.read_project_merge_rules())
        .await
        .map_err(|_| "后台项目合并设置读取任务意外结束。".to_owned())?
        .map_err(|error| match error {
            settings::SettingsError::InvalidProjectMergeRules => {
                "项目合并设置已损坏，当前未应用任何合并。".to_owned()
            }
            _ => "无法读取本机项目合并设置。".to_owned(),
        })
}

#[tauri::command(rename_all = "camelCase")]
async fn set_project_merge_rules(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
    rules: Vec<settings::ProjectMergeRule>,
) -> Result<Vec<settings::ProjectMergeRule>, String> {
    settings::validate_project_merge_rules(&rules)
        .map_err(|_| "项目合并规则无效，请检查成员和显示名称。".to_owned())?;
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.set_project_merge_rules(&rules))
        .await
        .map_err(|_| "后台项目合并设置保存任务意外结束。".to_owned())?
        .map_err(|error| match error {
            settings::SettingsError::InvalidProjectMergeRules => {
                "项目合并规则无效，请检查成员和显示名称。".to_owned()
            }
            _ => "无法保存本机项目合并设置。".to_owned(),
        })
}

#[tauri::command]
async fn read_pricing_settings(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
) -> Result<pricing::PricingSettings, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.read_pricing_settings())
        .await
        .map_err(|_| "后台模型定价读取任务意外结束。".to_owned())?
        .map_err(|error| match error {
            settings::SettingsError::InvalidPricingSettings => {
                "模型定价设置已损坏，当前不会套用任何价格。".to_owned()
            }
            _ => "无法读取本机模型定价设置。".to_owned(),
        })
}

#[tauri::command(rename_all = "camelCase")]
async fn set_pricing_settings(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
    pricing_settings: pricing::PricingSettings,
) -> Result<pricing::PricingSettings, String> {
    settings::validate_pricing_settings(&pricing_settings)
        .map_err(|_| "模型定价参数无效，请检查时间、倍率和单价。".to_owned())?;
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.set_pricing_settings(&pricing_settings))
        .await
        .map_err(|_| "后台模型定价保存任务意外结束。".to_owned())?
        .map_err(|error| match error {
            settings::SettingsError::InvalidPricingSettings => {
                "模型定价参数无效，请检查时间、倍率和单价。".to_owned()
            }
            _ => "无法保存本机模型定价设置。".to_owned(),
        })
}

#[tauri::command]
async fn send_test_notification(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, runtime::UsageRuntime>,
) -> Result<settings::AppPreferences, String> {
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        runtime
            .send_test_notification(&app)
            .map_err(|_| "无法发送测试通知；请确认应用已安装且系统允许通知。".to_owned())?;
        runtime
            .read_app_preferences(autostart_enabled)
            .map_err(|_| "测试通知已发送，但无法读取通知记录。".to_owned())
    })
    .await
    .map_err(|_| "后台通知任务意外结束。".to_owned())?
}

#[tauri::command]
async fn read_index_maintenance_report(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
) -> Result<runtime::IndexMaintenanceReport, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.index_maintenance_report())
        .await
        .map_err(|_| "后台索引自检任务意外结束。".to_owned())
}

#[tauri::command]
async fn rebuild_indexes(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, runtime::UsageRuntime>,
) -> Result<runtime::IndexRebuildResult, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let progress_app = app.clone();
        let result = runtime
            .rebuild_indexes_with_progress(|progress| {
                let _ = progress_app.emit(runtime::PROGRESS_EVENT, progress);
            })
            .map_err(|_| "无法完成索引重建；原始来源日志未被修改。如已创建维护备份，它会继续保留，可再次刷新或重建恢复派生索引。".to_owned())?;
        runtime::update_tray_tooltip(&app, &result.snapshot);
        Ok(result)
    })
    .await
    .map_err(|_| "后台索引重建任务意外结束。".to_owned())?
}

#[tauri::command(rename_all = "camelCase")]
async fn read_usage_day_detail(
    runtime: tauri::State<'_, runtime::UsageRuntime>,
    date: String,
) -> Result<multisource::UsageDayDetail, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.read_usage_day_detail(&date))
        .await
        .map_err(|_| "后台日明细读取任务意外结束。".to_owned())?
        .map_err(|_| "无法读取该日的本机用量明细。".to_owned())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _working_directory| {
                tray::show_main_window(app);
            },
        ))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            let codex_home = std::env::var_os("CODEX_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or(app.path().home_dir()?.join(".codex"));
            let usage_runtime = runtime::UsageRuntime::new(
                app.path().app_data_dir()?,
                codex_home,
                app.path().home_dir()?,
            );
            app.manage(usage_runtime.clone());

            let initial_autostart = app.autolaunch().is_enabled().unwrap_or(false);
            let tray_runtime = usage_runtime.clone();
            let tray_handle =
                tray::install(app, initial_autostart, move |app, action| match action {
                    tray::TrayAction::RefreshRequested => {
                        tray_runtime.request_event_refresh(app.clone());
                    }
                    tray::TrayAction::AutostartChanged { enabled } => {
                        let result = if enabled {
                            app.autolaunch().enable()
                        } else {
                            app.autolaunch().disable()
                        };
                        let effective = match result {
                            Ok(()) => enabled,
                            Err(_) => app.autolaunch().is_enabled().unwrap_or(!enabled),
                        };
                        if let Some(handle) = app.try_state::<tray::TrayMenuHandle<tauri::Wry>>() {
                            let _ = handle.set_autostart_enabled(effective);
                        }
                    }
                    tray::TrayAction::ExitRequested => tray_runtime.stop(),
                })?;
            app.manage(tray_handle);
            usage_runtime.start_scheduler(app.handle().clone());

            if std::env::args_os().any(|argument| argument == "--background") {
                if let Some(window) = app.get_webview_window(tray::MAIN_WINDOW_LABEL) {
                    let _ = window.hide();
                }
            }
            Ok(())
        })
        .on_window_event(tray::handle_window_event)
        .invoke_handler(tauri::generate_handler![
            read_dashboard_snapshot,
            read_cached_dashboard_snapshot,
            set_plan_renewal_at,
            read_usage_day_detail,
            read_visual_preferences,
            set_theme_preference,
            select_background_image,
            clear_background_image,
            read_app_preferences,
            set_app_preferences,
            read_project_merge_rules,
            set_project_merge_rules,
            read_pricing_settings,
            set_pricing_settings,
            send_test_notification,
            read_index_maintenance_report,
            rebuild_indexes
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
