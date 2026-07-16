//! CreateWaitableTimerEx wrappers, absolute-time, 30 s coalescing tolerance.
//! Exactly one edge timer armed at any moment.

use nudge_core::schedule::LocalNow;
use nudge_core::state::Event;
use nudge_core::UnixTime;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{
    CreateWaitableTimerExW, SetWaitableTimerEx, INFINITE, TIMER_ALL_ACCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    QS_ALLINPUT, WM_HOTKEY, WM_QUIT,
};

use crate::overlay::{self, PromptClick};
use crate::tray::{Tray, TrayCmd};

/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01 (Unix epoch).
const FILETIME_UNIX_OFFSET: i64 = 11_644_473_600;

pub struct EdgeTimer {
    handle: HANDLE,
}

impl EdgeTimer {
    pub fn new() -> Self {
        let handle = unsafe {
            CreateWaitableTimerExW(None, PCWSTR::null(), Default::default(), TIMER_ALL_ACCESS.0)
        }
        .expect("CreateWaitableTimerExW");
        Self { handle }
    }

    /// Raw handle for the message loop's wait set (valid for program lifetime).
    pub fn raw(&self) -> HANDLE {
        self.handle
    }

    /// SetWaitableTimerEx with absolute due time and 30_000 ms tolerable delay.
    /// Re-arming replaces the previous due time (never two timers).
    pub fn arm_absolute(&mut self, at: UnixTime) {
        let due: i64 = (at + FILETIME_UNIX_OFFSET) * 10_000_000; // 100 ns FILETIME units, positive = absolute
        unsafe {
            SetWaitableTimerEx(self.handle, &due, 0, None, None, None, 30_000)
                .expect("SetWaitableTimerEx");
        }
    }
}

impl Drop for EdgeTimer {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Local-time snapshot for nudge-core (owns the clock/TZ conversion).
pub fn local_now() -> LocalNow {
    let st = unsafe { GetLocalTime() };
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before 1970")
        .as_secs() as i64;
    LocalNow {
        unix,
        // SYSTEMTIME: 0 = sun .. 6 = sat -> core: 0 = mon .. 6 = sun.
        weekday: ((st.wDayOfWeek + 6) % 7) as u8,
        minutes: st.wHour as u32 * 60 + st.wMinute as u32,
    }
}

/// What the message loop hands back to main. `Core` events feed the pure state
/// machine directly; `Reload` is a request to re-read rules.toml from disk (file
/// I/O the loop stays out of).
pub enum LoopSignal {
    Core(Event),
    Reload,
    /// Notification click-through: launch/focus the nudge-app GUI and record the
    /// click. Carries no core `Event` — it's a pure svc-side side effect.
    OpenApp,
    /// Tray Pause (§6.6): the submenu's chosen duration rides along; `None`
    /// (the "Default (rules)" item) falls back to `[escalation] pause_secs`,
    /// which lives in rules — the loop deliberately doesn't hold them, so main
    /// stamps the fallback on. Break's duration is rules-only, same deal.
    Pause(UnixTime, Option<i64>),
    Break(UnixTime),
}

/// MsgWaitForMultipleObjects on {edge timer, quit event, reload event} + message
/// pump. Translates the timer signal, WM_HOTKEY, and tray commands into loop
/// signals; returns on quit (quit event set, tray Quit, or WM_QUIT).
pub fn message_loop(
    timer: HANDLE,
    quit: HANDLE,
    reload: HANDLE,
    tray: &Tray,
    mut on_signal: impl FnMut(LoopSignal),
) {
    loop {
        let wait = unsafe {
            MsgWaitForMultipleObjects(Some(&[timer, quit, reload]), false, INFINITE, QS_ALLINPUT)
        };
        if wait == WAIT_OBJECT_0 {
            on_signal(LoopSignal::Core(Event::EdgeTimer(local_now().unix)));
        } else if wait.0 == WAIT_OBJECT_0.0 + 1 {
            return; // quit event signaled (console handler or nudge-ctl quit)
        } else if wait.0 == WAIT_OBJECT_0.0 + 2 {
            on_signal(LoopSignal::Reload); // nudge-ctl edit signaled a reload
        }
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            match msg.message {
                WM_QUIT => return,
                WM_HOTKEY => on_signal(LoopSignal::Core(Event::HotkeyToggle(local_now().unix))),
                _ => unsafe {
                    // Tray callbacks dispatch to the tray-icon crate's window,
                    // which posts menu events to the channel drained below.
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                },
            }
        }
        // Drain menu clicks accumulated during message dispatch.
        while let Some(cmd) = tray.poll() {
            match cmd {
                TrayCmd::Toggle => {
                    on_signal(LoopSignal::Core(Event::HotkeyToggle(local_now().unix)))
                }
                TrayCmd::Pause(secs) => on_signal(LoopSignal::Pause(local_now().unix, secs)),
                TrayCmd::Resume => {
                    on_signal(LoopSignal::Core(Event::Resume(local_now().unix)))
                }
                TrayCmd::Reload => on_signal(LoopSignal::Reload),
                TrayCmd::Quit => return,
            }
        }
        // Drain prompt clicks: buttons → core events, a body click → OpenApp.
        while let Some(click) = overlay::poll_click() {
            let signal = match click {
                PromptClick::Start => LoopSignal::Core(Event::Ack(local_now().unix)),
                PromptClick::Snooze => LoopSignal::Core(Event::Snooze(local_now().unix)),
                PromptClick::Skip => LoopSignal::Core(Event::Skip(local_now().unix)),
                PromptClick::Yes => LoopSignal::Core(Event::CheckInYes(local_now().unix)),
                PromptClick::No => LoopSignal::Core(Event::CheckInNo(local_now().unix)),
                PromptClick::Break => LoopSignal::Break(local_now().unix),
                PromptClick::Open => LoopSignal::OpenApp,
                PromptClick::PickTask(id) => {
                    LoopSignal::Core(Event::PickTask(local_now().unix, id))
                }
                // The classify window owns the row-index → app-name mapping; a
                // row that no longer resolves (screen torn down between click
                // and drain) is dropped rather than misrouted.
                PromptClick::ClassifyRow(i, choice) => match crate::classify::app_at(i) {
                    Some(app_name) => LoopSignal::Core(Event::Classify {
                        at: local_now().unix,
                        app_name,
                        choice,
                    }),
                    None => continue,
                },
                PromptClick::ClassifyDone => {
                    LoopSignal::Core(Event::ClassifyDone(local_now().unix))
                }
            };
            on_signal(signal);
        }
    }
}
