//! The app window: full screen (`HostCommand::SetFullscreen` /
//! `HostCommand::ToggleFullscreen`).
//!
//! WebView2 doesn't map F11 or Alt+Enter to full screen the way a browser
//! does, so the webview catches those keys and calls [`toggle_fullscreen`]; an
//! agent reaches the same functions over the control socket. Every change is
//! announced as a [`FULLSCREEN_EVENT`], so the webview can remember the state
//! for the next launch whichever way it changed.

use tauri::{AppHandle, Emitter, Manager};

/// Event emitted with the new full-screen state (`bool`) after each change.
pub const FULLSCREEN_EVENT: &str = "fullscreen";

/// Put the main window in (`on`) or out of full screen and announce it.
pub fn apply_fullscreen(app: &AppHandle, on: bool) -> Result<bool, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "no main window".to_string())?;
    window.set_fullscreen(on).map_err(|e| e.to_string())?;
    let _ = app.emit(FULLSCREEN_EVENT, on);
    Ok(on)
}

/// Flip the main window's full screen and announce it. Returns the new state.
pub fn flip_fullscreen(app: &AppHandle) -> Result<bool, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "no main window".to_string())?;
    let on = !window.is_fullscreen().map_err(|e| e.to_string())?;
    apply_fullscreen(app, on)
}

/// Enter or leave full screen (`set_fullscreen`).
#[tauri::command]
pub fn set_fullscreen(app: AppHandle, on: bool) -> Result<bool, String> {
    apply_fullscreen(&app, on)
}

/// Toggle full screen: F11 / Alt+Enter (`toggle_fullscreen`).
#[tauri::command]
pub fn toggle_fullscreen(app: AppHandle) -> Result<bool, String> {
    flip_fullscreen(&app)
}
