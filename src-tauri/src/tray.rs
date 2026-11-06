use std::sync::Arc;

use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager, Runtime, Window, WindowEvent,
};

use crate::runtime::UsageRuntime;

pub const MAIN_WINDOW_LABEL: &str = "main";
pub const TRAY_ID: &str = "codex-usage-monitor";

const MENU_SHOW: &str = "tray-show";
const MENU_REFRESH: &str = "tray-refresh";
const MENU_AUTOSTART: &str = "tray-autostart";
const MENU_PET: &str = "tray-pet";
const MENU_EXIT: &str = "tray-exit";

/// Actions delegated to the application service layer by the tray menu.
///
/// The tray owns window visibility and the final `app.exit(0)` call. The
/// service callback should keep refresh/autostart work non-blocking and use
/// `ExitRequested` only for synchronous cleanup or a shutdown signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    RefreshRequested,
    AutostartChanged { enabled: bool },
    PetVisibilityChanged { enabled: bool },
    ExitRequested,
}

/// A small handle that lets the settings service reconcile the checkbox if
/// enabling or disabling autostart succeeds or fails asynchronously.
#[derive(Clone)]
pub struct TrayMenuHandle<R: Runtime> {
    autostart_item: CheckMenuItem<R>,
    pet_item: CheckMenuItem<R>,
}

impl<R: Runtime> TrayMenuHandle<R> {
    pub fn set_autostart_enabled(&self, enabled: bool) -> tauri::Result<()> {
        self.autostart_item.set_checked(enabled)
    }
    pub fn set_pet_enabled(&self, enabled: bool) -> tauri::Result<()> { self.pet_item.set_checked(enabled) }
}

/// Builds the native tray and returns a handle suitable for `app.manage(...)`.
///
/// `on_action` deliberately stays in Rust so the WebView never needs shell,
/// filesystem, SQL, process, tray, or autostart plugin permissions.
pub fn install<R, F>(
    app: &App<R>,
    autostart_enabled: bool,
    pet_enabled: bool,
    on_action: F,
) -> tauri::Result<TrayMenuHandle<R>>
where
    R: Runtime,
    F: Fn(&AppHandle<R>, TrayAction) + Send + Sync + 'static,
{
    let show_item = MenuItem::with_id(app, MENU_SHOW, "显示主窗口", true, None::<&str>)?;
    let refresh_item = MenuItem::with_id(app, MENU_REFRESH, "立即刷新", true, None::<&str>)?;
    let autostart_item = CheckMenuItem::with_id(
        app,
        MENU_AUTOSTART,
        "开机启动",
        true,
        autostart_enabled,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let pet_item = CheckMenuItem::with_id(app, MENU_PET, "显示桌面宠物", true, pet_enabled, None::<&str>)?;
    let exit_item = MenuItem::with_id(app, MENU_EXIT, "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &show_item,
            &refresh_item,
            &autostart_item,
            &pet_item,
            &separator,
            &exit_item,
        ],
    )?;

    let action_handler = Arc::new(on_action);
    let menu_action_handler = Arc::clone(&action_handler);
    let menu_autostart_item = autostart_item.clone();
    let menu_pet_item = pet_item.clone();

    let mut tray_builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("AI Token 用量监控")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app_handle, event| match event.id().as_ref() {
            MENU_SHOW => show_main_window(app_handle),
            MENU_REFRESH => {
                menu_action_handler(app_handle, TrayAction::RefreshRequested);
            }
            MENU_AUTOSTART => {
                if let Ok(enabled) = menu_autostart_item.is_checked() {
                    menu_action_handler(app_handle, TrayAction::AutostartChanged { enabled });
                }
            }
            MENU_PET => { if let Ok(enabled)=menu_pet_item.is_checked(){ menu_action_handler(app_handle,TrayAction::PetVisibilityChanged{enabled}); } }
            MENU_EXIT => {
                menu_action_handler(app_handle, TrayAction::ExitRequested);
                app_handle.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            let should_show = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );

            if should_show {
                show_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        tray_builder = tray_builder.icon(icon);
    }

    tray_builder.build(app)?;

    Ok(TrayMenuHandle { autostart_item, pet_item })
}

/// Shows and focuses the single configured dashboard window.
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Wire this into `tauri::Builder::on_window_event` so the title-bar close
/// button hides the dashboard while the Rust background worker keeps running.
pub fn handle_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW_LABEL {
        return;
    }

    if let WindowEvent::CloseRequested { api, .. } = event {
        let runtime = window.app_handle().try_state::<UsageRuntime>();
        let hide_to_tray = runtime
            .as_ref()
            .map_or(true, |runtime| runtime.hide_on_close());
        if hide_to_tray {
            api.prevent_close();
            let _ = window.hide();
        } else if let Some(runtime) = runtime {
            api.prevent_close();
            runtime.stop();
            window.app_handle().exit(0);
        }
    }
}
