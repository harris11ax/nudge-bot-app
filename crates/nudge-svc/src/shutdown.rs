//! Graceful shutdown signaling. A named, manual-reset Win32 event
//! (`Local\nudge-bot-quit`) is added to the message-loop wait set; setting it
//! ends the loop so Resources/tray/overlay Drop on the main (creating) thread —
//! clean NIM_DELETE, no orphaned tray icon. Two producers set it:
//!   * the console control handler (Ctrl+C / window close / logoff / shutdown),
//!   * `nudge-ctl quit`, which opens the same event by name (see `signal`).
//!
//! For close/logoff/shutdown the OS terminates the process the instant the
//! handler returns, so the handler blocks (bounded) on a `done` event that
//! `mark_done` sets after teardown — long enough to remove the tray icon,
//! short enough to stay inside the OS grace window.

use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE, TRUE};
use windows::Win32::System::Console::{
    SetConsoleCtrlHandler, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

/// Session-local name shared by the service (creator) and nudge-ctl (opener,
/// which duplicates this literal — keep the two in sync).
const NAME: PCWSTR = w!("Local\\nudge-bot-quit");

/// Handles reached by the console-control handler, which runs on its own
/// thread. Stored as isize because HANDLE is a raw pointer (not Sync). 0 = unset.
static QUIT_EVENT: AtomicIsize = AtomicIsize::new(0);
static DONE_EVENT: AtomicIsize = AtomicIsize::new(0);

fn handle_from(raw: isize) -> HANDLE {
    HANDLE(raw as *mut core::ffi::c_void)
}

pub struct QuitSignal {
    quit: HANDLE,
    done: HANDLE,
}

impl QuitSignal {
    /// Create the named quit event + an internal done event, and install the
    /// console control handler. Call once on the message-loop thread at boot.
    pub fn install() -> Self {
        unsafe {
            let quit = CreateEventW(None, TRUE, BOOL(0), NAME).expect("CreateEventW quit");
            let done = CreateEventW(None, TRUE, BOOL(0), PCWSTR::null()).expect("CreateEventW done");
            QUIT_EVENT.store(quit.0 as isize, Ordering::SeqCst);
            DONE_EVENT.store(done.0 as isize, Ordering::SeqCst);
            SetConsoleCtrlHandler(Some(ctrl_handler), TRUE).expect("SetConsoleCtrlHandler");
            Self { quit, done }
        }
    }

    /// Handle for the message loop's wait set. Signaled -> loop returns.
    pub fn raw(&self) -> HANDLE {
        self.quit
    }

    /// Release a console handler blocking the OS shutdown grace window. Call
    /// after tray/overlay teardown so the icon is gone before the process dies.
    pub fn mark_done(&self) {
        unsafe {
            let _ = SetEvent(self.done);
        }
    }
}

impl Drop for QuitSignal {
    fn drop(&mut self) {
        QUIT_EVENT.store(0, Ordering::SeqCst);
        DONE_EVENT.store(0, Ordering::SeqCst);
        unsafe {
            let _ = CloseHandle(self.quit);
            let _ = CloseHandle(self.done);
        }
    }
}

/// Console control handler (own thread). Sets the quit event to wake the loop,
/// then for terminal events waits on `done` so teardown finishes first.
unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> BOOL {
    let quit = QUIT_EVENT.load(Ordering::SeqCst);
    if quit != 0 {
        let _ = SetEvent(handle_from(quit));
    }
    if matches!(ctrl_type, CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT) {
        let done = DONE_EVENT.load(Ordering::SeqCst);
        if done != 0 {
            let _ = WaitForSingleObject(handle_from(done), 4000);
        }
    }
    TRUE // handled
}
