//! Launch-a-tool-from-a-task (UI-PLAN §7.4c). The task page can spin up the apps
//! and websites a task needs, on demand — one tool or the whole set.
//!
//! A `task_tools` row is either a **website** or a **Windows application**:
//!   * an explicit bound `url` (stored on the row) → open that URL;
//!   * else a per-site entry (`<exe> → <host>`, §7.4b) with no bound URL →
//!     open `https://<host>` in the default browser;
//!   * else a bare exe name → `ShellExecute` it (App Paths / PATH resolves a
//!     bare `code.exe`, mirroring the svc's Tier-C switch in `nudge-svc`).
//!
//! The exe path is skipped when the app is already running (a website always
//! opens — a new tab is cheap and the "the URL I need" intent is explicit). The
//! resolver below is pure and unit-tested; `ShellExecute` and the process
//! snapshot are the only Windows-only glue.

/// What a `(app_name, url)` tool row resolves to for launching.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// Open this URL in the default browser (`ShellExecute open`).
    Web(String),
    /// `ShellExecute` this exe (skipped if already running).
    Exe(String),
}

/// The per-site separator §7.4b writes into browser tool names: `<exe> → <host>`.
const SITE_SEP: &str = " → ";

/// Resolve a tool row to a launch [`Target`] (pure — the single source of the
/// website-vs-exe rule):
///   1. a non-blank bound `url` → [`Target::Web`] verbatim;
///   2. a per-site `<exe> → <host>` name → [`Target::Web`] `https://<host>`;
///   3. otherwise the bare name is an exe → [`Target::Exe`].
pub fn resolve_target(app_name: &str, url: Option<&str>) -> Target {
    if let Some(u) = url.map(str::trim).filter(|u| !u.is_empty()) {
        return Target::Web(u.to_string());
    }
    if let Some((_exe, host)) = app_name.split_once(SITE_SEP) {
        let host = host.trim();
        if !host.is_empty() {
            return Target::Web(format!("https://{host}"));
        }
    }
    Target::Exe(app_name.trim().to_string())
}

/// Skip an exe launch when a process of the same name is already running
/// (case-insensitive, mirroring the svc switch). Websites never reach here.
pub fn exe_is_running(exe: &str, running: &[String]) -> bool {
    running.iter().any(|r| r.eq_ignore_ascii_case(exe))
}

#[cfg(windows)]
mod win {
    use super::Target;
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    /// `ShellExecute("open", target)`. Returns `Ok(())` on success; per the
    /// ShellExecute contract a return value `<= 32` is an error code.
    pub fn shell_open(target: &str) -> Result<(), String> {
        let ret = unsafe {
            ShellExecuteW(
                None,
                &HSTRING::from("open"),
                &HSTRING::from(target),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if ret.0 as isize <= 32 {
            Err(format!("ShellExecute failed (code {}) for {target}", ret.0 as isize))
        } else {
            Ok(())
        }
    }

    /// Perform one resolved launch: open a URL, or ShellExecute an exe unless it
    /// is already in `running`.
    pub fn launch(target: &Target, running: &[String]) -> Result<(), String> {
        match target {
            Target::Web(url) => shell_open(url),
            Target::Exe(exe) => {
                if super::exe_is_running(exe, running) {
                    Ok(()) // already running — nothing to do
                } else {
                    shell_open(exe)
                }
            }
        }
    }

    /// Exe names of all running processes (Toolhelp snapshot). Empty on failure,
    /// which errs toward launching — matching "the user asked to open this".
    pub fn running_exes() -> Vec<String> {
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
}

#[cfg(windows)]
pub use win::{launch, running_exes};

/// Non-Windows stub so the crate builds off-Windows (tests for the pure resolver
/// still run everywhere). Reports the launch as unsupported.
#[cfg(not(windows))]
pub fn running_exes() -> Vec<String> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn launch(_target: &Target, _running: &[String]) -> Result<(), String> {
    Err("launch is only supported on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_url_wins() {
        assert_eq!(
            resolve_target("brave.exe → github.com", Some("https://github.com/me/repo")),
            Target::Web("https://github.com/me/repo".into())
        );
        // blank/whitespace url is ignored — falls through to the host rule
        assert_eq!(
            resolve_target("brave.exe → github.com", Some("  ")),
            Target::Web("https://github.com".into())
        );
    }

    #[test]
    fn per_site_name_becomes_https_host() {
        assert_eq!(
            resolve_target("chrome.exe → docs.rs", None),
            Target::Web("https://docs.rs".into())
        );
    }

    #[test]
    fn bare_exe_is_exe() {
        assert_eq!(resolve_target("code.exe", None), Target::Exe("code.exe".into()));
        // an exe with an empty host after the separator is treated as an exe name,
        // not a bogus `https://` (defensive — shouldn't occur from §7.4b).
        assert_eq!(
            resolve_target("weird.exe → ", None),
            Target::Exe("weird.exe →".into())
        );
    }

    #[test]
    fn running_skip_is_case_insensitive() {
        let running = vec!["CODE.EXE".to_string(), "explorer.exe".into()];
        assert!(exe_is_running("code.exe", &running));
        assert!(!exe_is_running("word.exe", &running));
    }
}
