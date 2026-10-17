//! Window lifecycle: single instance, close confirmation and window geometry (owner decisions Q6 and Q8).
//!
//! * **Single instance** (`tauri-plugin-single-instance`): a second launch exits immediately and the running
//!   instance's main window is restored and focused ([`focus_main_window`]).
//! * **Close while busy**: closing the main window while the live driver pipeline records or streams, or while an
//!   engineer is connected to a driver, is held and a native confirm dialog is shown (from Rust via
//!   `tauri-plugin-dialog`, so it works even if the WebView is unresponsive; no dialog permission is granted to the
//!   WebView). "Quit" exits the app; `RunEvent::Exit` then runs the strict core shutdown (recording finalised,
//!   clients stopped). "Keep running" leaves everything as it was. There is no tray: nothing keeps running after
//!   the window closes.
//! * **Window geometry** (`config.window`): restored at start-up if it is still on a connected monitor, saved when
//!   the window closes.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow, Window, WindowEvent,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use app_core::bridge::status::{AppMode, CoreStatus, Origin};

use crate::config::WindowConfig;
use crate::logging::component;
use crate::state::AppState;

/// Label of the main window (tauri.conf.json).
pub const MAIN_WINDOW: &str = "main";

/// Recorder state while it writes.
const RECORDING_ACTIVE: &str = "active";

/// Something that would be interrupted by quitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseBlocker {
    Recording,
    Streaming,
    EngineerConnected,
}

impl CloseBlocker {
    fn describe(self) -> &'static str {
        match self {
            CloseBlocker::Recording => {
                "the session is being recorded (the recording is finalised safely)"
            }
            CloseBlocker::Streaming => "telemetry is streaming to your engineers",
            CloseBlocker::EngineerConnected => {
                "you are connected to a driver (reconnecting needs a new code)"
            }
        }
    }
}

/// What quitting now would interrupt. `status` is the latest pipeline status; it only counts while the live LMU
/// driver pipeline runs (the demo is display-only and never records or streams).
pub fn close_blockers(
    status: Option<&CoreStatus>,
    live_driver_running: bool,
    engineer_connected: bool,
) -> Vec<CloseBlocker> {
    let mut blockers = Vec::new();
    if live_driver_running
        && let Some(status) =
            status.filter(|s| s.mode == AppMode::Driver && s.origin == Origin::Lmu)
    {
        if status.recording.state == RECORDING_ACTIVE {
            blockers.push(CloseBlocker::Recording);
        }
        if status.network.streaming || status.network.engineers_connected > 0 {
            blockers.push(CloseBlocker::Streaming);
        }
    }
    if engineer_connected {
        blockers.push(CloseBlocker::EngineerConnected);
    }
    blockers
}

/// Text of the confirm dialog.
pub fn confirm_text(blockers: &[CloseBlocker]) -> String {
    let mut text = String::from("Quitting now will stop everything:\n");
    for blocker in blockers {
        text.push_str("\n• ");
        text.push_str(blocker.describe());
    }
    text.push_str("\n\nQuit LMU Race Engineering?");
    text
}

/// Flags of the close flow (managed state).
#[derive(Debug, Default)]
pub struct CloseGuard {
    dialog_open: AtomicBool,
    quit_confirmed: AtomicBool,
}

/// Restore, show and focus the main window (second-instance launch).
pub fn focus_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        tracing::warn!(
            component = component::APP,
            "second instance: main window not found"
        );
        return;
    };
    let result = window
        .unminimize()
        .and_then(|()| window.show())
        .and_then(|()| window.set_focus());
    if let Err(error) = result {
        tracing::warn!(component = component::APP, error = %error, "second instance: could not focus the window");
    }
    tracing::info!(
        component = component::APP,
        "second launch redirected to the running instance"
    );
}

/// Window event hook: close confirmation and geometry saving for the main window.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW {
        return;
    }
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    let app = window.app_handle().clone();
    let (Some(state), Some(guard)) = (app.try_state::<AppState>(), app.try_state::<CloseGuard>())
    else {
        return;
    };

    if guard.quit_confirmed.load(Ordering::SeqCst) {
        save_geometry(window, &state);
        return;
    }

    let core = state.core();
    let live_driver_running = core.is_driver_running() && !core.is_demo_running();
    let blockers = close_blockers(
        Some(&core.status()),
        live_driver_running,
        core.engineer_client().is_some(),
    );
    if blockers.is_empty() {
        save_geometry(window, &state);
        return;
    }

    api.prevent_close();
    if guard.dialog_open.swap(true, Ordering::SeqCst) {
        // A dialog is already showing; ignore repeated close clicks.
        return;
    }
    tracing::info!(
        component = component::APP,
        ?blockers,
        "close requested while busy; asking for confirmation"
    );

    let callback_app = app.clone();
    let callback_window = window.clone();
    let mut dialog = app
        .dialog()
        .message(confirm_text(&blockers))
        .title("Quit LMU Race Engineering?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Quit".to_owned(),
            "Keep running".to_owned(),
        ));
    if let Some(webview) = app.get_webview_window(MAIN_WINDOW) {
        dialog = dialog.parent(&webview);
    }
    dialog.show(move |quit| {
        let Some(guard) = callback_app.try_state::<CloseGuard>() else {
            return;
        };
        guard.dialog_open.store(false, Ordering::SeqCst);
        if !quit {
            tracing::info!(component = component::APP, "quit cancelled by the user");
            return;
        }
        guard.quit_confirmed.store(true, Ordering::SeqCst);
        if let Some(state) = callback_app.try_state::<AppState>() {
            save_geometry(&callback_window, &state);
        }
        tracing::info!(component = component::APP, "quit confirmed by the user");
        // RunEvent::Exit runs the strict core shutdown before the process ends.
        callback_app.exit(0);
    });
}

/// Geometry worth saving: the restored (non-maximised) bounds; while maximised only the flag changes; while
/// minimised nothing changes (Windows reports a parking position of -32000).
pub fn geometry_to_save(
    previous: &WindowConfig,
    minimized: bool,
    maximized: bool,
    position: (i32, i32),
    size: (u32, u32),
) -> WindowConfig {
    if minimized {
        return previous.clone();
    }
    if maximized {
        return WindowConfig {
            maximized: true,
            ..previous.clone()
        };
    }
    WindowConfig {
        x: Some(position.0),
        y: Some(position.1),
        width: Some(size.0),
        height: Some(size.1),
        maximized: false,
    }
}

fn save_geometry<R: Runtime>(window: &Window<R>, state: &AppState) {
    let read = || -> tauri::Result<WindowConfig> {
        let position = window.outer_position()?;
        let size = window.inner_size()?;
        Ok(geometry_to_save(
            &state.window_geometry(),
            window.is_minimized()?,
            window.is_maximized()?,
            (position.x, position.y),
            (size.width, size.height),
        ))
    };
    match read() {
        Ok(geometry) => {
            if let Err(error) = state.save_window_geometry(geometry) {
                tracing::warn!(component = component::CONFIG, error = %error, "window geometry not saved");
            }
        }
        Err(error) => {
            tracing::warn!(component = component::CONFIG, error = %error, "window geometry not readable");
        }
    }
}

/// A monitor's area in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// How much of the saved title-bar area must be on a monitor for the position to be restored.
const VISIBLE_MARGIN_PX: i64 = 64;

/// Whether a window at `(x, y)` keeps at least a grabbable strip of its top edge on one of `monitors`.
pub fn position_is_visible(x: i32, y: i32, width: u32, monitors: &[MonitorRect]) -> bool {
    let (x, y, width) = (i64::from(x), i64::from(y), i64::from(width));
    monitors.iter().any(|m| {
        let (mx, my) = (i64::from(m.x), i64::from(m.y));
        let (mr, mb) = (mx + i64::from(m.width), my + i64::from(m.height));
        let overlap = (x + width).min(mr) - x.max(mx);
        overlap >= VISIBLE_MARGIN_PX && y >= my && y + VISIBLE_MARGIN_PX / 2 <= mb
    })
}

/// Apply the saved geometry to the main window. Failures are logged, never fatal.
pub fn restore_window_geometry<R: Runtime>(window: &WebviewWindow<R>, saved: &WindowConfig) {
    let result = (|| -> tauri::Result<()> {
        if let (Some(width), Some(height)) = (saved.width, saved.height) {
            window.set_size(PhysicalSize::new(width, height))?;
        }
        if let (Some(x), Some(y)) = (saved.x, saved.y) {
            let width = saved.width.unwrap_or(0).max(VISIBLE_MARGIN_PX as u32);
            let monitors: Vec<MonitorRect> = window
                .available_monitors()?
                .iter()
                .map(|m| MonitorRect {
                    x: m.position().x,
                    y: m.position().y,
                    width: m.size().width,
                    height: m.size().height,
                })
                .collect();
            if position_is_visible(x, y, width, &monitors) {
                window.set_position(PhysicalPosition::new(x, y))?;
            } else {
                tracing::info!(
                    component = component::CONFIG,
                    "saved window position is off-screen; centring"
                );
            }
        }
        if saved.maximized {
            window.maximize()?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        tracing::warn!(component = component::CONFIG, error = %error, "window geometry not restored");
    }
}

#[cfg(test)]
mod tests {
    use app_core::bridge::status::{
        DropsDto, LmuSourceStatus, NetworkStatusDto, RatesDto, RecordingStatusDto, SessionStatusDto,
    };

    use super::*;

    fn status(recording: &str, network: &str, engineers: u16, origin: Origin) -> CoreStatus {
        CoreStatus {
            mode: AppMode::Driver,
            origin,
            lmu: LmuSourceStatus {
                state: "LIVE".to_owned(),
                detail: None,
                game_version: None,
            },
            session: SessionStatusDto {
                session_id: None,
                session_type: None,
                track: None,
                car: None,
                vehicle_class: None,
                started_at_unix_s: None,
            },
            recording: RecordingStatusDto {
                state: recording.to_owned(),
                recording_id: None,
                bytes_written: 0,
                frames_written: 0,
                frames_lost: 0,
                disk_free_bytes: None,
                error: None,
            },
            network: NetworkStatusDto {
                // "STREAMING" in these tests means: connected and streaming.
                state: if network == "STREAMING" {
                    "CONNECTED".to_owned()
                } else {
                    network.to_owned()
                },
                streaming: network == "STREAMING",
                rtt_ms: None,
                connect_code: None,
                code_expires_at_unix_s: None,
                engineers_connected: engineers,
                engineers_max: 2,
                degraded_reason: None,
                driver_away: false,
                last_error: None,
                security_issue: false,
                failure: None,
                relogin_required: false,
                engineers: Vec::new(),
            },
            rates: RatesDto {
                telemetry_read_hz: None,
                scoring_read_hz: None,
                frames_sent_hz: None,
                bytes_sent_per_s: None,
                frames_dropped_total: 0,
            },
            drops: DropsDto::default(),
            engineer: None,
        }
    }

    #[test]
    fn idle_app_closes_without_confirmation() {
        assert!(close_blockers(None, false, false).is_empty());
        assert!(
            close_blockers(None, true, false).is_empty(),
            "no status yet: nothing recorded"
        );
        let waiting = status("off", "DISCONNECTED", 0, Origin::Lmu);
        assert!(close_blockers(Some(&waiting), true, false).is_empty());
    }

    #[test]
    fn recording_streaming_and_engineer_sessions_need_confirmation() {
        let busy = status(RECORDING_ACTIVE, "STREAMING", 0, Origin::Lmu);
        assert_eq!(
            close_blockers(Some(&busy), true, false),
            vec![CloseBlocker::Recording, CloseBlocker::Streaming]
        );
        // Streaming stopped, but an engineer still holds a slot in the kept relay session.
        let engineers = status("off", "DISCONNECTED", 1, Origin::Lmu);
        assert_eq!(
            close_blockers(Some(&engineers), true, false),
            vec![CloseBlocker::Streaming]
        );
        assert_eq!(
            close_blockers(None, false, true),
            vec![CloseBlocker::EngineerConnected]
        );
        let text = confirm_text(&[CloseBlocker::Recording, CloseBlocker::EngineerConnected]);
        assert!(text.contains("recorded") && text.contains("new code"));
    }

    #[test]
    fn stale_or_synthetic_status_never_blocks() {
        let busy = status(RECORDING_ACTIVE, "STREAMING", 2, Origin::Lmu);
        assert!(
            close_blockers(Some(&busy), false, false).is_empty(),
            "pipeline stopped"
        );
        let demo = status(RECORDING_ACTIVE, "STREAMING", 2, Origin::Synthetic);
        assert!(
            close_blockers(Some(&demo), true, false).is_empty(),
            "demo is display-only"
        );
    }

    #[test]
    fn geometry_saving_keeps_restored_bounds() {
        let previous = WindowConfig {
            x: Some(10),
            y: Some(20),
            width: Some(1400),
            height: Some(900),
            maximized: false,
        };
        assert_eq!(
            geometry_to_save(&previous, true, false, (-32_000, -32_000), (160, 28)),
            previous
        );
        let maximized = geometry_to_save(&previous, false, true, (-8, -8), (2560, 1400));
        assert_eq!(
            maximized,
            WindowConfig {
                maximized: true,
                ..previous.clone()
            }
        );
        let moved = geometry_to_save(&previous, false, false, (100, 50), (1200, 800));
        assert_eq!(
            moved,
            WindowConfig {
                x: Some(100),
                y: Some(50),
                width: Some(1200),
                height: Some(800),
                maximized: false
            }
        );
    }

    #[test]
    fn off_screen_positions_are_not_restored() {
        let monitors = [
            MonitorRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            MonitorRect {
                x: 1920,
                y: 0,
                width: 2560,
                height: 1440,
            },
        ];
        assert!(position_is_visible(100, 100, 1200, &monitors));
        assert!(
            position_is_visible(3000, 200, 1200, &monitors),
            "second monitor"
        );
        assert!(
            !position_is_visible(5000, 100, 1200, &monitors),
            "monitor unplugged"
        );
        assert!(
            !position_is_visible(100, -500, 1200, &monitors),
            "title bar above the screen"
        );
        assert!(
            !position_is_visible(-1190, 100, 1200, &monitors),
            "only 10 px visible"
        );
        assert!(!position_is_visible(100, 100, 1200, &[]));
    }
}
