//! Notification click-through → launch or focus the nudge-app GUI (UI-P1, 9d-ii).
//!
//! The overlay strip's body (everything left of the `[Start][Snooze][Skip]`
//! cluster) is a click target: clicking it surfaces the task's page in
//! nudge-app. This module is the svc side of that. Single-instance policy is
//! "focus if already running, else spawn":
//!
//!   * A running instance is found by its top-level window (title
//!     [`APP_WINDOW_TITLE`], set in nudge-app's `tauri.conf.json`). We bring it
//!     to the foreground and hand it the target task id via `WM_COPYDATA`.
//!   * If no window is found, we spawn `nudge-app.exe` (resolved as a sibling of
//!     the running svc exe) with `--task <id>`.
//!
//! Everything here is best-effort: a missing exe, an unreadable exe path, or an
//! app that ignores `WM_COPYDATA` (until the app-side receiver lands — see the
//! 9d-ii carry-forward) degrades to a no-op / plain focus, never a svc crash.
//! The `WM_COPYDATA` payload and CLI grammar are the app-side contract; the pure
//! builders below are the single source of that wire format and are unit-tested.

use std::process::Command;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SendMessageTimeoutW, SetForegroundWindow, SMTO_ABORTIFHUNG, WM_COPYDATA,
};

/// Top-level window title of a running nudge-app. MUST match the `main` window
/// `title` in `crates/nudge-app/src-tauri/tauri.conf.json`; kept in sync by hand
/// (same discipline as the duplicated svc/ctl event-name literals).
const APP_WINDOW_TITLE: &str = "nudge-bot";

/// Filename of the GUI executable, resolved as a sibling of the svc exe.
const APP_EXE: &str = "nudge-app.exe";

/// Non-zero tag stamped into the `WM_COPYDATA` `dwData` so the app can tell a
/// nudge focus-request apart from any other cross-process message ('ND').
const COPYDATA_TASK: usize = 0x4E44;

/// CLI args to spawn the GUI with: `--task <id>` when a task is known, else empty
/// (open to the default page).
fn build_args(task_id: Option<i64>) -> Vec<String> {
    match task_id {
        Some(id) => vec!["--task".into(), id.to_string()],
        None => Vec::new(),
    }
}

/// `WM_COPYDATA` string payload handed to a running instance: `task:<id>` when a
/// task is known, else empty (plain focus).
fn copydata_payload(task_id: Option<i64>) -> String {
    match task_id {
        Some(id) => format!("task:{id}"),
        None => String::new(),
    }
}

/// Resolve the GUI exe as a sibling of the running svc exe. `None` if the
/// current-exe path can't be read (the best-effort spawn then skips silently).
fn app_exe_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(APP_EXE))
}

/// Focus a running nudge-app or spawn a new one, targeting `task_id` (P1
/// notification click-through). Best-effort: every failure is logged and
/// swallowed so a stray click can never take the svc down.
pub fn open_app(task_id: Option<i64>) {
    let hwnd = unsafe {
        FindWindowW(PCWSTR::null(), &HSTRING::from(APP_WINDOW_TITLE)).unwrap_or_default()
    };
    if !hwnd.is_invalid() {
        unsafe { focus_and_notify(hwnd, task_id) };
        return;
    }
    match app_exe_path() {
        Some(path) => {
            if let Err(e) = Command::new(&path).args(build_args(task_id)).spawn() {
                eprintln!("open_app: spawn {} failed: {e}", path.display());
            }
        }
        None => eprintln!("open_app: cannot resolve current exe path"),
    }
}

/// Bring an existing window to the foreground and hand it the task payload via
/// `WM_COPYDATA`. `SendMessageTimeoutW` (`SMTO_ABORTIFHUNG`, 1 s) so a wedged GUI
/// can't stall the svc message loop. The app-side receiver is not wired yet
/// (9d-ii carry-forward); until then this still focuses the window and the
/// message is harmlessly ignored.
unsafe fn focus_and_notify(hwnd: HWND, task_id: Option<i64>) {
    let _ = SetForegroundWindow(hwnd);
    let mut payload: Vec<u8> = copydata_payload(task_id).into_bytes();
    let cds = COPYDATASTRUCT {
        dwData: COPYDATA_TASK,
        cbData: payload.len() as u32,
        lpData: payload.as_mut_ptr() as *mut _,
    };
    let _ = SendMessageTimeoutW(
        hwnd,
        WM_COPYDATA,
        WPARAM(0),
        LPARAM(&cds as *const _ as isize),
        SMTO_ABORTIFHUNG,
        1000,
        None,
    );
}

/// Tier-C row switch (§6.5/P4): launch the picked task's tools that aren't
/// already running. Tool names are exe filenames (`task_tools.app_name`, the
/// same form AW reports); `ShellExecuteW` resolves them via PATH / App Paths,
/// so a bare "code.exe" launches without a stored full path. Best-effort per
/// exe — a tool that fails to resolve is logged and skipped, never a crash.
pub fn launch_tools(db: &crate::persist::Db, task_id: i64) {
    let tools: Vec<String> = db
        .task_tools(task_id)
        .into_iter()
        .filter(|(_, kind)| kind == "tool")
        .map(|(name, _)| name)
        .collect();
    for exe in tools_to_launch(&tools, &running_exes()) {
        let ret = unsafe {
            windows::Win32::UI::Shell::ShellExecuteW(
                HWND::default(),
                &HSTRING::from("open"),
                &HSTRING::from(exe.as_str()),
                PCWSTR::null(),
                PCWSTR::null(),
                windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
            )
        };
        // Per ShellExecute contract, values <= 32 are error codes.
        if ret.0 as isize <= 32 {
            eprintln!("launch_tools: could not launch {exe} (code {})", ret.0 as isize);
        }
    }
}

/// The pure filter behind [`launch_tools`]: which of `tools` are not in
/// `running` (case-insensitive exe-name compare, mirroring the on-task
/// compares)? Split out so the skip-running rule is unit-testable without a
/// process snapshot.
fn tools_to_launch(tools: &[String], running: &[String]) -> Vec<String> {
    tools
        .iter()
        .filter(|t| !running.iter().any(|r| r.eq_ignore_ascii_case(t)))
        .cloned()
        .collect()
}

/// Exe names of all running processes (Toolhelp snapshot). Empty on failure —
/// which errs toward launching, matching "the user asked to switch".
fn running_exes() -> Vec<String> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    let snap = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        Ok(h) => h,
        Err(_) => return out,
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    if unsafe { Process32FirstW(snap, &mut entry) }.is_ok() {
        loop {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            out.push(String::from_utf16_lossy(&entry.szExeFile[..len]));
            if unsafe { Process32NextW(snap, &mut entry) }.is_err() {
                break;
            }
        }
    }
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // D.3: the switch launches only what isn't already running, matching exe
    // names case-insensitively.
    #[test]
    fn launch_filter_skips_running_exes() {
        let tools = vec!["code.exe".to_string(), "word.exe".into(), "fig.exe".into()];
        let running = vec!["CODE.EXE".to_string(), "explorer.exe".into()];
        assert_eq!(tools_to_launch(&tools, &running), vec!["word.exe", "fig.exe"]);
        // Nothing running (snapshot failed) → launch everything asked for.
        assert_eq!(tools_to_launch(&tools, &[]).len(), 3);
        assert!(tools_to_launch(&[], &running).is_empty());
    }

    #[test]
    fn args_with_task() {
        assert_eq!(build_args(Some(7)), vec!["--task".to_string(), "7".to_string()]);
    }

    #[test]
    fn args_without_task() {
        assert!(build_args(None).is_empty());
    }

    #[test]
    fn payload_with_task() {
        assert_eq!(copydata_payload(Some(42)), "task:42");
    }

    #[test]
    fn payload_without_task() {
        assert_eq!(copydata_payload(None), "");
    }
}
