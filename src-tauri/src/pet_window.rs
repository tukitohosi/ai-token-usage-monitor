use crate::pets::{PetPosition, PetRuntime, PetState};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, Window,
};

pub(crate) const PET_WINDOW: &str = "pet";
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
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
    sprite_position: PhysicalPosition<f64>,
}
#[derive(Default)]
pub(crate) struct PetWindowRuntime {
    layout: Mutex<PetLayout>,
    drag: Mutex<Option<Drag>>,
    ready: AtomicBool,
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
    let runtime = app.state::<PetWindowRuntime>();
    if !runtime.ready.load(Ordering::Acquire) {
        let position = state
            .preferences
            .position
            .map(|p| PhysicalPosition::new(p.x, p.y))
            .unwrap_or_else(|| default_position(&window));
        position_on_top(&window.as_ref().window(), Some(position))?;
        return configure_native(&window);
    }
    let layout = *runtime.layout.lock().unwrap_or_else(|p| p.into_inner());
    arrange_window(&window, &runtime, layout, None)?;
    window.show().map_err(window_error)?;
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

// Clamp the sprite and the attached bar independently, so the bar never pushes the pet inward.
fn arrange_layout(
    sprite_position: PhysicalPosition<f64>,
    measured: PetLayout,
    area: Rect,
    scale: f64,
) -> (PetLayout, PhysicalPosition<i32>) {
    let sw = measured.sprite.width * scale;
    let sh = measured.sprite.height * scale;
    let bw = measured.bar.width * scale;
    let bh = measured.bar.height * scale;
    let clamp = |value: f64, origin: f64, extent: f64, size: f64| {
        value.clamp(origin, origin + (extent - size).max(0.))
    };
    let sx = clamp(sprite_position.x, area.x, area.width, sw);
    let sy = clamp(sprite_position.y, area.y, area.height, sh);
    let bx = clamp(sx + (sw - bw) / 2., area.x, area.width, bw);
    let below = sy + sh + 4. * scale;
    let above = sy - bh - 4. * scale;
    let by = if below + bh > area.y + area.height && above >= area.y {
        above
    } else {
        clamp(below, area.y, area.height, bh)
    };
    let position = PhysicalPosition::new(sx.min(bx).floor() as i32, sy.min(by).floor() as i32);
    let rect = |x: f64, y: f64, width: f64, height: f64| Rect {
        x: (x - position.x as f64) / scale,
        y: (y - position.y as f64) / scale,
        width: width / scale,
        height: height / scale,
    };
    (
        PetLayout {
            width: ((sx + sw).max(bx + bw) - position.x as f64) / scale,
            height: ((sy + sh).max(by + bh) - position.y as f64) / scale,
            sprite: rect(sx, sy, sw, sh),
            bar: rect(bx, by, bw, bh),
        },
        position,
    )
}

fn arrange_window(
    window: &WebviewWindow,
    runtime: &PetWindowRuntime,
    measured: PetLayout,
    sprite_position: Option<PhysicalPosition<f64>>,
) -> Result<PetLayout, String> {
    let previous = *runtime.layout.lock().unwrap_or_else(|p| p.into_inner());
    let position = window.outer_position().map_err(window_error)?;
    let scale = window.scale_factor().map_err(window_error)?;
    let anchor = sprite_position.unwrap_or_else(|| {
        PhysicalPosition::new(
            position.x as f64 + previous.sprite.x * scale,
            position.y as f64 + previous.sprite.y * scale,
        )
    });
    let center = (
        anchor.x + measured.sprite.width * scale / 2.,
        anchor.y + measured.sprite.height * scale / 2.,
    );
    let monitor = window
        .monitor_from_point(center.0, center.1)
        .ok()
        .flatten()
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());
    let (layout, position) = if let Some(monitor) = monitor {
        let area = monitor.work_area();
        arrange_layout(
            anchor,
            measured,
            Rect {
                x: area.position.x as f64,
                y: area.position.y as f64,
                width: area.size.width as f64,
                height: area.size.height as f64,
            },
            scale,
        )
    } else {
        (measured, position)
    };
    *runtime.layout.lock().unwrap_or_else(|p| p.into_inner()) = layout;
    if previous != layout || !runtime.ready.load(Ordering::Acquire) {
        window
            .set_size(PhysicalSize::new(
                (layout.width * scale).ceil() as u32,
                (layout.height * scale).ceil() as u32,
            ))
            .map_err(window_error)?;
        configure_native(window)?;
        update_region(window, layout)?;
        window.emit("pet://layout", layout).map_err(window_error)?;
    }
    position_on_top(&window.as_ref().window(), Some(position))?;
    Ok(layout)
}

fn save_window_position(app: &AppHandle, window: &WebviewWindow) -> Result<(), String> {
    let position = window.outer_position().map_err(window_error)?;
    let scale = window.scale_factor().map_err(window_error)?;
    let layout = *app
        .state::<PetWindowRuntime>()
        .layout
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let initial = PetLayout::default();
    if let Some(pets) = app.try_state::<PetRuntime>() {
        pets.save_position(PetPosition {
            x: (position.x as f64 + (layout.sprite.x - initial.sprite.x) * scale).round() as i32,
            y: (position.y as f64 + (layout.sprite.y - initial.sprite.y) * scale).round() as i32,
        })?;
    }
    Ok(())
}

pub(crate) fn reset_position(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(PET_WINDOW) {
        position_on_top(&window.as_ref().window(), Some(default_position(&window)))?;
        let runtime = app.state::<PetWindowRuntime>();
        let layout = *runtime.layout.lock().unwrap_or_else(|p| p.into_inner());
        arrange_window(&window, &runtime, layout, None)?;
        save_window_position(app, &window)?;
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
) -> Result<PetLayout, String> {
    require_pet(&window)?;
    if !valid_layout(layout) {
        return Err("宠物窗口布局无效。".into());
    }
    let window = app
        .get_webview_window(PET_WINDOW)
        .ok_or("宠物窗口尚未准备好。")?;
    let layout = arrange_window(&window, &runtime, layout, None)?;
    runtime.ready.store(true, Ordering::Release);
    if app.state::<PetRuntime>().state().preferences.enabled {
        window.show().map_err(window_error)?;
        configure_native(&window)?;
        update_region(&window, layout)?;
    }
    if runtime
        .drag
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_none()
    {
        save_window_position(&app, &window)?;
    }
    Ok(layout)
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
    position_on_top(&window, None)?;
    *runtime.drag.lock().unwrap_or_else(|p| p.into_inner()) = Some(Drag {
        cursor,
        sprite_position: PhysicalPosition::new(
            position.x as f64 + sprite.x * scale,
            position.y as f64 + sprite.y * scale,
        ),
    });
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
        let window = app
            .get_webview_window(PET_WINDOW)
            .ok_or("宠物窗口尚未准备好。")?;
        let layout = *runtime.layout.lock().unwrap_or_else(|p| p.into_inner());
        arrange_window(
            &window,
            &runtime,
            layout,
            Some(PhysicalPosition::new(
                drag.sprite_position.x + cursor.x - drag.cursor.x,
                drag.sprite_position.y + cursor.y - drag.cursor.y,
            )),
        )?;
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
    position_on_top(&window, None)?;
    if let Some(window) = app.get_webview_window(PET_WINDOW) {
        save_window_position(&app, &window)?;
        if let Some(pets) = app.try_state::<PetRuntime>() {
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
                    if let Ok(layout) = arrange_window(&window, &runtime, layout, None) {
                        let _ = configure_native(&window);
                        let _ = update_region(&window, layout);
                    }
                }
            });
        }
        _ => {}
    }
}

#[cfg(windows)]
fn configure_native(window: &WebviewWindow) -> Result<(), String> {
    let owned = window.clone();
    on_ui(&window.as_ref().window(), move || {
        configure_native_on_ui(&owned)
    })
}
#[cfg(windows)]
fn on_ui(
    window: &Window,
    operation: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    // Wry executes run_on_main_thread inline when called on the window thread.
    // This also allows synchronous drag IPC without waiting on its own queue.
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
    let hwnd = window.hwnd().map_err(window_error)?;
    remove_native_frame(hwnd)?;
    unsafe {
        if !SetWindowSubclass(hwnd, Some(no_activate), 0x41545045, 0).as_bool() {
            return Err("无法设置宠物窗口的非激活交互。".into());
        }
    }
    topmost_on_ui(hwnd, None)
}

#[cfg(windows)]
fn remove_native_frame(hwnd: windows::Win32::Foundation::HWND) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION,
        WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_WINDOWEDGE, WS_THICKFRAME,
    };
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let extended = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        // Tao keeps WS_CAPTION on undecorated top-level windows; tool-window changes can repaint it.
        let borderless = style & !((WS_CAPTION | WS_THICKFRAME).0 as isize);
        let tool = (extended
            & !((WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_DLGMODALFRAME).0 as isize))
            | (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE).0 as isize;
        if style != borderless || extended != tool {
            SetWindowLongPtrW(hwnd, GWL_STYLE, borderless);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, tool);
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
            )
            .map_err(|_| "无法移除宠物窗口标题栏。".to_string())?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn position_on_top(window: &Window, position: Option<PhysicalPosition<i32>>) -> Result<(), String> {
    let owned = window.clone();
    on_ui(window, move || {
        topmost_on_ui(owned.hwnd().map_err(window_error)?, position)
    })
}

#[cfg(windows)]
fn topmost_on_ui(
    hwnd: windows::Win32::Foundation::HWND,
    position: Option<PhysicalPosition<i32>>,
) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
    };
    let mut flags = SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSIZE;
    let position = position.unwrap_or_else(|| {
        flags |= SWP_NOMOVE;
        PhysicalPosition::new(0, 0)
    });
    // Moving with SWP_NOZORDER would retain a stale order. Reassert the native
    // topmost group on every drag move, without activating the pet or its owner.
    unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            position.x,
            position.y,
            0,
            0,
            flags,
        )
        .map_err(|_| "无法维持桌面宠物的置顶显示。".to_string())
    }
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
    on_ui(&window.as_ref().window(), move || {
        update_region_on_ui(&owned, layout)
    })
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
fn position_on_top(window: &Window, position: Option<PhysicalPosition<i32>>) -> Result<(), String> {
    if let Some(position) = position {
        window.set_position(position).map_err(window_error)?;
    }
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

    #[test]
    fn placement_keeps_sprite_at_all_edges_and_bar_inside_work_area() {
        let measured = PetLayout::default();
        let area = Rect {
            x: -1920.,
            y: -200.,
            width: 1920.,
            height: 1080.,
        };
        for scale in [1., 1.25, 1.5, 2.] {
            let sw = measured.sprite.width * scale;
            let sh = measured.sprite.height * scale;
            for x in [area.x, area.x + 600., area.x + area.width - sw] {
                for y in [area.y, area.y + 250., area.y + area.height - sh] {
                    let (layout, position) =
                        arrange_layout(PhysicalPosition::new(x, y), measured, area, scale);
                    assert!((position.x as f64 + layout.sprite.x * scale - x).abs() < 0.001);
                    assert!((position.y as f64 + layout.sprite.y * scale - y).abs() < 0.001);
                    assert!(valid_layout(layout));
                    for rect in [layout.sprite, layout.bar] {
                        let left = position.x as f64 + rect.x * scale;
                        let top = position.y as f64 + rect.y * scale;
                        assert!(left >= area.x && top >= area.y);
                        assert!(left + rect.width * scale <= area.x + area.width + 0.001);
                        assert!(top + rect.height * scale <= area.y + area.height + 0.001);
                    }
                    if y == area.y + area.height - sh {
                        assert!(layout.bar.y < layout.sprite.y);
                    }
                }
            }
        }
    }

    #[test]
    fn placement_recovers_a_pet_from_a_disconnected_monitor() {
        let area = Rect {
            x: 0.,
            y: 0.,
            width: 1920.,
            height: 1040.,
        };
        let (layout, position) = arrange_layout(
            PhysicalPosition::new(-4000., 3000.),
            PetLayout::default(),
            area,
            1.,
        );
        assert_eq!(position.x as f64 + layout.sprite.x, 0.);
        assert_eq!(
            position.y as f64 + layout.sprite.y + layout.sprite.height,
            1040.
        );
        assert!(layout.bar.y < layout.sprite.y);
    }

    #[cfg(windows)]
    #[test]
    fn native_moves_restore_topmost_without_activating_or_resizing() {
        use windows::{
            core::w,
            Win32::{
                Foundation::RECT,
                UI::WindowsAndMessaging::{
                    CreateWindowExW, DestroyWindow, GetClientRect, GetForegroundWindow,
                    GetWindowLongPtrW, GetWindowRect, SetWindowPos, GWL_EXSTYLE, GWL_STYLE,
                    HWND_NOTOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_CAPTION,
                    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_THICKFRAME,
                },
            },
        };
        unsafe {
            let foreground = GetForegroundWindow();
            // A hidden test window exercises Z-order without drawing over the desktop.
            let hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("AI Token pet window test"),
                WS_POPUP | WS_CAPTION | WS_THICKFRAME,
                50,
                70,
                20,
                30,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            remove_native_frame(hwnd).unwrap();
            assert_eq!(
                GetWindowLongPtrW(hwnd, GWL_STYLE) & (WS_CAPTION | WS_THICKFRAME).0 as isize,
                0
            );
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client).unwrap();
            assert_eq!(
                (client.right - client.left, client.bottom - client.top),
                (20, 30)
            );
            topmost_on_ui(hwnd, Some(PhysicalPosition::new(150, 170))).unwrap();
            let mut rect = RECT::default();
            GetWindowRect(hwnd, &mut rect).unwrap();
            assert_eq!(
                (
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top
                ),
                (150, 170, 20, 30)
            );
            assert_ne!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as isize,
                0
            );
            SetWindowPos(
                hwnd,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )
            .unwrap();
            topmost_on_ui(hwnd, None).unwrap();
            let mut restored = RECT::default();
            GetWindowRect(hwnd, &mut restored).unwrap();
            assert_eq!(
                (restored.left, restored.top, restored.right, restored.bottom),
                (rect.left, rect.top, rect.right, rect.bottom)
            );
            assert_ne!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as isize,
                0
            );
            assert_eq!(GetForegroundWindow(), foreground);
            DestroyWindow(hwnd).unwrap();
        }
    }
}
