use crate::pets::{PetPosition, PetRuntime, PetState};
use serde::Deserialize;
use std::sync::Mutex;
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, Window,
};

pub(crate) const PET_WINDOW: &str = "pet";
#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct PetLayout {
    pub width: f64,
    pub height: f64,
    pub sprite: Rect,
    pub bar: Rect,
}
impl Default for PetLayout {
    fn default() -> Self {
        Self {
            width: 280.,
            height: 225.,
            sprite: Rect {
                x: 76.,
                y: 0.,
                width: 128.,
                height: 139.,
            },
            bar: Rect {
                x: 0.,
                y: 143.,
                width: 280.,
                height: 82.,
            },
        }
    }
}
#[derive(Clone, Copy)]
struct Drag {
    cursor: PhysicalPosition<f64>,
    position: PhysicalPosition<i32>,
}
#[derive(Default)]
pub(crate) struct PetWindowRuntime {
    layout: Mutex<PetLayout>,
    drag: Mutex<Option<Drag>>,
}

fn require_pet(window: &Window) -> Result<(), String> {
    if window.label() == PET_WINDOW {
        Ok(())
    } else {
        Err("此操作仅供宠物窗口使用。".into())
    }
}
fn window_error(_: tauri::Error) -> String {
    "无法更新桌面宠物窗口。".into()
}

pub(crate) fn apply(app: &AppHandle, state: &PetState) -> Result<(), String> {
    if let Some(runtime) = app.try_state::<PetRuntime>() {
        runtime.allow_assets(app, state)?;
    }
    if !state.preferences.enabled {
        if let Some(window) = app.get_webview_window(PET_WINDOW) {
            window.hide().map_err(window_error)?;
        }
        return Ok(());
    }
    let window = match app.get_webview_window(PET_WINDOW) {
        Some(window) => window,
        None => WebviewWindowBuilder::new(
            app,
            PET_WINDOW,
            WebviewUrl::App("index.html?window=pet".into()),
        )
        .title("AI Token 桌面宠物")
        .inner_size(280., 225.)
        .resizable(false)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .visible(false)
        .background_color(tauri::window::Color(0, 0, 0, 0))
        .build()
        .map_err(window_error)?,
    };
    let position = state
        .preferences
        .position
        .map(|p| PhysicalPosition::new(p.x, p.y))
        .unwrap_or_else(|| default_position(&window));
    window.set_position(position).map_err(window_error)?;
    clamp_window(&window)?;
    if let Some(runtime) = app.try_state::<PetWindowRuntime>() {
        update_region(
            &window,
            *runtime.layout.lock().unwrap_or_else(|p| p.into_inner()),
        )?;
    }
    window.show().map_err(window_error)?;
    // Tao may rewrite extended styles when showing a window. Apply the final
    // tool-window/no-activation styles after every show (initial or restored).
    configure_native(&window)
}

fn default_position(window: &WebviewWindow) -> PhysicalPosition<i32> {
    let monitor = window.primary_monitor().ok().flatten();
    match monitor {
        Some(m) => {
            let area = m.work_area();
            let size = window.outer_size().unwrap_or(PhysicalSize::new(280, 225));
            PhysicalPosition::new(
                area.position.x + (area.size.width.saturating_sub(size.width + 32)) as i32,
                area.position.y + (area.size.height.saturating_sub(size.height + 32)) as i32,
            )
        }
        None => PhysicalPosition::new(80, 80),
    }
}

fn clamp_window(window: &WebviewWindow) -> Result<PetPosition, String> {
    let position = window.outer_position().map_err(window_error)?;
    let size = window.outer_size().map_err(window_error)?;
    let center = (
        position.x as f64 + size.width as f64 / 2.,
        position.y as f64 + size.height as f64 / 2.,
    );
    let monitor = window
        .monitor_from_point(center.0, center.1)
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten());
    let corrected = if let Some(m) = monitor {
        let a = m.work_area();
        PhysicalPosition::new(
            position.x.clamp(
                a.position.x,
                a.position.x + (a.size.width.saturating_sub(size.width)) as i32,
            ),
            position.y.clamp(
                a.position.y,
                a.position.y + (a.size.height.saturating_sub(size.height)) as i32,
            ),
        )
    } else {
        position
    };
    if position != corrected {
        window.set_position(corrected).map_err(window_error)?;
    }
    Ok(PetPosition {
        x: corrected.x,
        y: corrected.y,
    })
}

pub(crate) fn reset_position(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(PET_WINDOW) {
        window
            .set_position(default_position(&window))
            .map_err(window_error)?;
        let position = clamp_window(&window)?;
        if let Some(runtime) = app.try_state::<PetRuntime>() {
            runtime.save_position(position)?;
        }
    }
    Ok(())
}

fn valid_layout(layout: PetLayout) -> bool {
    let finite = |v: f64| v.is_finite() && (0.0..=1000.).contains(&v);
    [
        layout.width,
        layout.height,
        layout.sprite.x,
        layout.sprite.y,
        layout.sprite.width,
        layout.sprite.height,
        layout.bar.x,
        layout.bar.y,
        layout.bar.width,
        layout.bar.height,
    ]
    .into_iter()
    .all(finite)
        && layout.width >= 96.
        && layout.height >= 96.
        && [layout.sprite, layout.bar].into_iter().all(|r| {
            r.width > 0.
                && r.height > 0.
                && r.x + r.width <= layout.width + 0.1
                && r.y + r.height <= layout.height + 0.1
        })
}

#[tauri::command]
pub(crate) async fn set_pet_layout(
    app: AppHandle,
    window: Window,
    layout: PetLayout,
    runtime: tauri::State<'_, PetWindowRuntime>,
) -> Result<(), String> {
    require_pet(&window)?;
    if !valid_layout(layout) {
        return Err("宠物窗口布局无效。".into());
    }
    *runtime.layout.lock().unwrap_or_else(|p| p.into_inner()) = layout;
    let window = app
        .get_webview_window(PET_WINDOW)
        .ok_or("宠物窗口尚未准备好。")?;
    let scale = window.scale_factor().map_err(window_error)?;
    window
        .set_size(PhysicalSize::new(
            (layout.width * scale).ceil() as u32,
            (layout.height * scale).ceil() as u32,
        ))
        .map_err(window_error)?;
    update_region(&window, layout)?;
    configure_native(&window)?;
    let position = clamp_window(&window)?;
    if let Some(pets) = app.try_state::<PetRuntime>() {
        pets.save_position(position)?;
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn pet_drag_start(
    window: Window,
    runtime: tauri::State<'_, PetWindowRuntime>,
    client_x: f64,
    client_y: f64,
) -> Result<(), String> {
    require_pet(&window)?;
    let sprite = runtime
        .layout
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .sprite;
    if !client_x.is_finite()
        || !client_y.is_finite()
        || !(sprite.x..=sprite.x + sprite.width).contains(&client_x)
        || !(sprite.y..=sprite.y + sprite.height).contains(&client_y)
    {
        return Err("请从宠物图像区域开始拖动。".into());
    }
    let position = window.outer_position().map_err(window_error)?;
    let scale = window.scale_factor().map_err(window_error)?;
    // Preserve the pointer-down location even when IPC arrives after a fast drag.
    // Subsequent moves still use the native global cursor, in physical pixels.
    let cursor = PhysicalPosition::new(
        position.x as f64 + client_x * scale,
        position.y as f64 + client_y * scale,
    );
    *runtime.drag.lock().unwrap_or_else(|p| p.into_inner()) = Some(Drag { cursor, position });
    Ok(())
}
#[tauri::command]
pub(crate) fn pet_drag_update(
    app: AppHandle,
    window: Window,
    runtime: tauri::State<'_, PetWindowRuntime>,
) -> Result<(), String> {
    require_pet(&window)?;
    let drag = *runtime.drag.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(drag) = drag {
        let cursor = app.cursor_position().map_err(window_error)?;
        window
            .set_position(PhysicalPosition::new(
                drag.position.x + (cursor.x - drag.cursor.x).round() as i32,
                drag.position.y + (cursor.y - drag.cursor.y).round() as i32,
            ))
            .map_err(window_error)?;
    }
    Ok(())
}
#[tauri::command]
pub(crate) fn pet_drag_end(
    app: AppHandle,
    window: Window,
    runtime: tauri::State<'_, PetWindowRuntime>,
) -> Result<(), String> {
    require_pet(&window)?;
    *runtime.drag.lock().unwrap_or_else(|p| p.into_inner()) = None;
    if let Some(window) = app.get_webview_window(PET_WINDOW) {
        let position = clamp_window(&window)?;
        if let Some(pets) = app.try_state::<PetRuntime>() {
            pets.save_position(position)?;
            pets.publish(&app);
        }
    }
    Ok(())
}

pub(crate) fn handle_window_event(window: &Window, event: &tauri::WindowEvent) {
    if window.label() != PET_WINDOW {
        return;
    }
    match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let _ = window.hide();
            if let Some(pets) = window.app_handle().try_state::<PetRuntime>() {
                let mut prefs = pets.state().preferences;
                prefs.enabled = false;
                if pets.save_preferences(prefs).is_ok() {
                    pets.publish(window.app_handle());
                }
            }
        }
        tauri::WindowEvent::ScaleFactorChanged { .. } => {
            let app = window.app_handle().clone();
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let (Some(window), Some(runtime)) = (
                    handle.get_webview_window(PET_WINDOW),
                    handle.try_state::<PetWindowRuntime>(),
                ) {
                    let layout = *runtime.layout.lock().unwrap_or_else(|p| p.into_inner());
                    if let Ok(scale) = window.scale_factor() {
                        let _ = window.set_size(PhysicalSize::new(
                            (layout.width * scale).ceil() as u32,
                            (layout.height * scale).ceil() as u32,
                        ));
                    }
                    let _ = update_region(&window, layout);
                    let _ = configure_native(&window);
                    let _ = clamp_window(&window);
                }
            });
        }
        _ => {}
    }
}

#[cfg(windows)]
fn configure_native(window: &WebviewWindow) -> Result<(), String> {
    let owned = window.clone();
    on_ui(window, move || configure_native_on_ui(&owned))
}
#[cfg(windows)]
fn on_ui(
    window: &WebviewWindow,
    operation: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    window
        .run_on_main_thread(move || {
            let _ = tx.send(operation());
        })
        .map_err(window_error)?;
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "宠物窗口更新超时。".to_string())?
}
#[cfg(windows)]
fn configure_native_on_ui(window: &WebviewWindow) -> Result<(), String> {
    use windows::Win32::UI::Shell::SetWindowSubclass;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    let hwnd = window.hwnd().map_err(window_error)?;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            style | WS_EX_TOOLWINDOW.0 as isize | WS_EX_NOACTIVATE.0 as isize,
        );
        if !SetWindowSubclass(hwnd, Some(no_activate), 0x41545045, 0).as_bool() {
            return Err("无法设置宠物窗口的非激活交互。".into());
        }
    }
    Ok(())
}
#[cfg(windows)]
unsafe extern "system" fn no_activate(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    w: windows::Win32::Foundation::WPARAM,
    l: windows::Win32::Foundation::LPARAM,
    _id: usize,
    _data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::{
        Foundation::LRESULT,
        UI::{
            Shell::DefSubclassProc,
            WindowsAndMessaging::{MA_NOACTIVATE, WM_MOUSEACTIVATE},
        },
    };
    if msg == WM_MOUSEACTIVATE {
        LRESULT(MA_NOACTIVATE as isize)
    } else {
        DefSubclassProc(hwnd, msg, w, l)
    }
}
#[cfg(windows)]
fn update_region(window: &WebviewWindow, layout: PetLayout) -> Result<(), String> {
    let owned = window.clone();
    on_ui(window, move || update_region_on_ui(&owned, layout))
}
#[cfg(windows)]
fn update_region_on_ui(window: &WebviewWindow, layout: PetLayout) -> Result<(), String> {
    use windows::Win32::Graphics::Gdi::{
        CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_OR,
    };
    let scale = window.scale_factor().map_err(window_error)?;
    let hwnd = window.hwnd().map_err(window_error)?;
    let make = |r: Rect| unsafe {
        CreateRectRgn(
            (r.x * scale).floor() as i32,
            (r.y * scale).floor() as i32,
            ((r.x + r.width) * scale).ceil() as i32,
            ((r.y + r.height) * scale).ceil() as i32,
        )
    };
    unsafe {
        let sprite = make(layout.sprite);
        let bar = make(layout.bar);
        if sprite.0.is_null() || bar.0.is_null() {
            if !sprite.0.is_null() {
                let _ = DeleteObject(sprite.into());
            }
            if !bar.0.is_null() {
                let _ = DeleteObject(bar.into());
            }
            return Err("无法建立宠物窗口区域。".into());
        }
        let combined = CombineRgn(Some(sprite), Some(sprite), Some(bar), RGN_OR);
        let _ = DeleteObject(bar.into());
        if combined.0 == 0 || SetWindowRgn(hwnd, Some(sprite), true) == 0 {
            let _ = DeleteObject(sprite.into());
            return Err("无法设置宠物窗口区域。".into());
        }
        // A successful SetWindowRgn transfers the region to Windows.
    }
    Ok(())
}
#[cfg(not(windows))]
fn configure_native(_: &WebviewWindow) -> Result<(), String> {
    Ok(())
}
#[cfg(not(windows))]
fn update_region(_: &WebviewWindow, _: PetLayout) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_rejects_non_finite_and_outside_rects() {
        assert!(valid_layout(PetLayout::default()));
        let mut layout = PetLayout::default();
        layout.sprite.x = f64::NAN;
        assert!(!valid_layout(layout));
        layout = PetLayout::default();
        layout.bar.width = 500.;
        assert!(!valid_layout(layout));
    }
}
