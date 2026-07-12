//! External reload signaling. A named, auto-reset Win32 event
//! (`Local\nudge-bot-reload`) added to the message-loop wait set; setting it
//! makes the loop re-read rules.toml from disk (same path as the tray "Reload
//! rules" item). Producer: `nudge-ctl` after a successful config edit, which
//! opens the same event by name (duplicating this literal — keep in sync).
//!
//! Auto-reset (bManualReset = FALSE): MsgWaitForMultipleObjects clears it on
//! return, so one SetEvent = one reload.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE};
use windows::Win32::System::Threading::CreateEventW;

/// Session-local name shared by the service (creator) and nudge-ctl (opener,
/// which duplicates this literal — keep the two in sync).
const NAME: PCWSTR = w!("Local\\nudge-bot-reload");

pub struct ReloadSignal {
    event: HANDLE,
}

impl ReloadSignal {
    /// Create the named auto-reset reload event. Call once on the message-loop
    /// thread at boot.
    pub fn install() -> Self {
        let event = unsafe {
            CreateEventW(None, /* manual reset */ BOOL(0), /* initial */ BOOL(0), NAME)
        }
        .expect("CreateEventW reload");
        Self { event }
    }

    /// Handle for the message loop's wait set. Signaled -> loop reloads rules.
    pub fn raw(&self) -> HANDLE {
        self.event
    }
}

impl Drop for ReloadSignal {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}
