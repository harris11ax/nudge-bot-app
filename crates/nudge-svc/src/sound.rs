//! Escalation alert sound. The pure state machine emits `Effect::PlaySound`
//! only on an L2 re-alert tick (see `escalate::Step::realert`); the svc plays
//! the system exclamation asynchronously so the message loop never blocks.
//!
//! `MessageBeep` is dependency-free (no winmm/PlaySound) and honours the user's
//! sound-scheme + mute settings — the polite default for a background nudger.

use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::WindowsAndMessaging::MB_ICONWARNING;

/// Fire the exclamation alert. Best-effort: a failed/absent sound scheme is not
/// worth surfacing, so the error is swallowed.
pub fn alert() {
    unsafe {
        let _ = MessageBeep(MB_ICONWARNING);
    }
}
