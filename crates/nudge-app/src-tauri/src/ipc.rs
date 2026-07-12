//! App-side receiver for svc click-through focus (9d-ii). Two entry points feed
//! the same `nudge://open-task` frontend event:
//!
//!   * Cold start: svc spawns us with `--task <id>` (see svc `launch::build_args`).
//!     [`cli_task_id`] parses it; the frontend pulls it once via `get_pending_task`.
//!   * Already running: svc finds our window and sends `WM_COPYDATA` (see svc
//!     `launch::copydata_payload` / `COPYDATA_TASK`). [`install_copydata_handler`]
//!     subclasses the main window's `WNDPROC` to catch it and emits the event
//!     directly, since the frontend is already listening by then.
//!
//! The wire format (`dwData` tag, `task:<id>` string payload) is frozen svc-side
//! and duplicated here by necessity — there's no shared crate between the two
//! Tauri-less/Tauri binaries. Keep in sync with `crates/nudge-svc/src/launch.rs`.

use std::sync::OnceLock;

use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, SetWindowLongPtrW, GWLP_WNDPROC, WM_COPYDATA, WNDPROC,
};

/// Must match svc `launch::COPYDATA_TASK` ('ND').
const COPYDATA_TASK: usize = 0x4E44;

/// Tauri event name the frontend listens on (App.svelte) to route to a task's
/// detail page.
pub const OPEN_TASK_EVENT: &str = "nudge://open-task";

static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
static ORIG_WNDPROC: OnceLock<isize> = OnceLock::new();

/// Parse `--task <id>` from the process's own argv. `None` if absent or
/// malformed (plain launch, opens to the default page).
pub fn cli_task_id() -> Option<i64> {
    let args: Vec<String> = std::env::args().collect();
    let idx = args.iter().position(|a| a == "--task")?;
    parse_task_payload(&format!("task:{}", args.get(idx + 1)?))
}

/// Parse the `WM_COPYDATA` string payload (`task:<id>`, or empty for a plain
/// focus with no task).
fn parse_task_payload(s: &str) -> Option<i64> {
    s.strip_prefix("task:")?.trim().parse().ok()
}

/// Subclass the main window's `WNDPROC` so a `WM_COPYDATA` from svc (already-
/// running instance) reaches us. Best-effort: if the main window isn't found
/// yet (shouldn't happen — called from `setup`, after window creation) this is
/// a silent no-op, matching the "never take the app down" discipline of the
/// svc-side sender.
pub fn install_copydata_handler(app: &AppHandle) {
    let _ = APP_HANDLE.set(app.clone());
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    unsafe {
        let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, wndproc as *const () as isize);
        let _ = ORIG_WNDPROC.set(prev);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA {
        handle_copydata(lparam);
    }
    let orig: WNDPROC = std::mem::transmute(ORIG_WNDPROC.get().copied().unwrap_or_default());
    CallWindowProcW(orig, hwnd, msg, wparam, lparam)
}

/// Extract the task id (if any) from a `WM_COPYDATA` message and emit
/// [`OPEN_TASK_EVENT`], then bring the window to front. A malformed or
/// tag-mismatched message is ignored, not an error — best-effort IPC.
unsafe fn handle_copydata(lparam: LPARAM) {
    let cds = &*(lparam.0 as *const COPYDATASTRUCT);
    if cds.dwData != COPYDATA_TASK {
        return;
    }
    let bytes = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
    let Ok(payload) = std::str::from_utf8(bytes) else {
        return;
    };
    let Some(app) = APP_HANDLE.get() else { return };
    if let Some(id) = parse_task_payload(payload) {
        let _ = app.emit(OPEN_TASK_EVENT, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_with_task() {
        assert_eq!(parse_task_payload("task:42"), Some(42));
    }

    #[test]
    fn payload_without_task() {
        assert_eq!(parse_task_payload(""), None);
    }

    #[test]
    fn payload_malformed() {
        assert_eq!(parse_task_payload("task:abc"), None);
    }
}
