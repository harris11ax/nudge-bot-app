//! TEMPORARY diagnostic probe for the Google `not_configured`-on-user-launch bug
//! (PLAN-google-oauth-launch.md, Phase 1). Appends one self-identifying line per
//! event to `%TEMP%\nudge-google-debug.log` so the user's double-click/.vbs launch
//! and a shell launch can be diffed. Every write is guarded (`let _ =`) — the probe
//! never panics and never leaks a handle (open/write/drop in one call).
//!
//! REMOVE at Phase 5 along with the log file: delete this module, its `mod`
//! declaration in `lib.rs`, and the `probe::*` call sites, and drop the
//! `Win32_System_Diagnostics_ToolHelp` Cargo feature added for it.

use std::sync::OnceLock;

/// Random-ish per-process nonce so every log line names which launch wrote it,
/// even when two processes interleave in the same file. Derived once from
/// address-space + PID entropy (no rand dep, good enough to distinguish launches).
static NONCE: OnceLock<u32> = OnceLock::new();

fn nonce() -> u32 {
    *NONCE.get_or_init(|| {
        let pid = std::process::id();
        let stack_marker = &pid as *const u32 as usize as u32;
        pid.rotate_left(13) ^ stack_marker ^ 0x9E37_79B9
    })
}

/// Seconds since the unix epoch as a bare number — cheap, no chrono formatting
/// needed for a diff.
fn ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn log_path() -> std::path::PathBuf {
    let dir = std::env::var("TEMP")
        .or_else(|_| std::env::var("TMP"))
        .unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(dir).join("nudge-google-debug.log")
}

/// Append one line, fully guarded. `stage` names the event; `fields` is the
/// caller-formatted payload. Every line is prefixed with nonce/pid/parent so it
/// self-identifies its launch.
pub fn probe_log(stage: &str, fields: &str) {
    use std::io::Write;
    let line = format!(
        "ts={} nonce={:08x} pid={} parent={} stage={} {}\n",
        ts(),
        nonce(),
        std::process::id(),
        parent_process_name(),
        stage,
        fields,
    );
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

/// Best-effort parent-process image name via a ToolHelp snapshot. Returns
/// `"?"` on any failure — the probe must never fail a `google_status` call.
#[cfg(windows)]
fn parent_process_name() -> String {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let me = std::process::id();
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return "?".to_string(),
        };
        // Snapshot handle closes when `snap` (a HANDLE wrapper) drops? No — HANDLE
        // is a raw wrapper; close explicitly below.
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        // Pass 1: find our parent's PID.
        let mut parent_pid: Option<u32> = None;
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                if entry.th32ProcessID == me {
                    parent_pid = Some(entry.th32ParentProcessID);
                    break;
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }

        // Pass 2: map parent PID -> image name.
        let mut name = "?".to_string();
        if let Some(ppid) = parent_pid {
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut entry).is_ok() {
                loop {
                    if entry.th32ProcessID == ppid {
                        let len = entry
                            .szExeFile
                            .iter()
                            .position(|&c| c == 0)
                            .unwrap_or(entry.szExeFile.len());
                        name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                        break;
                    }
                    if Process32NextW(snap, &mut entry).is_err() {
                        break;
                    }
                }
            }
        }

        let _ = windows::Win32::Foundation::CloseHandle(snap);
        name
    }
}

#[cfg(not(windows))]
fn parent_process_name() -> String {
    "?".to_string()
}

/// Startup line so parentage is captured even if `google_status` is delayed
/// (Phase 1 step 3).
pub fn probe_startup() {
    let localappdata = match std::env::var("LOCALAPPDATA") {
        Ok(v) => format!("Ok({v})"),
        Err(e) => format!("Err({e})"),
    };
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|e| format!("Err({e})"));
    probe_log(
        "startup",
        &format!("localappdata={localappdata:?} cwd={cwd:?}"),
    );
}
