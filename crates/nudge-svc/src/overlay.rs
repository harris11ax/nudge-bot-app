//! Topmost intention-anchor strip. Layered window:
//! WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW. GDI text render,
//! repaint on WM_PAINT / update(). Destroyed (not hidden) when dropped.
//!
//! Escalation visibility (L0/L1/L2) rides in GWLP_USERDATA so the WndProc paints
//! the strip's background from it. The right edge carries three hit-tested
//! buttons `[Start][Snooze][Skip]`; a left-click posts a [`PromptClick`] to a
//! global channel that the message loop drains via [`poll_click`] (mirrors the
//! tray-icon `poll()` seam).

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, Once, OnceLock};

use nudge_core::escalate::Level;
use nudge_core::state::Buttons;
use nudge_core::Mode;
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect,
    InvalidateRect, SetBkMode, SetTextColor, DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER,
    PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, RegisterClassW,
    SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowTextW, ShowWindow, GWLP_USERDATA,
    LWA_ALPHA, SM_CXSCREEN, SM_CYSCREEN, SW_SHOWNOACTIVATE, WM_LBUTTONDOWN, WM_PAINT, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const FG: COLORREF = COLORREF(0x00F0_F0F0);
/// Muted foreground for the on-task peripheral render (low-contrast, unobtrusive).
const FG_MUTED: COLORREF = COLORREF(0x00A0_A0A0);
/// Per-level strip background (0x00BBGGRR): gentle grey, amber, alarm red.
const BG_L0: COLORREF = COLORREF(0x0020_2020);
const BG_L1: COLORREF = COLORREF(0x0014_96E6); // amber
const BG_L2: COLORREF = COLORREF(0x0020_20C8); // red
/// On-task background: a single flat muted charcoal, no escalation coloring —
/// the on-task cue never climbs the ladder (UI-PLAN §1).
const BG_ONTASK: COLORREF = COLORREF(0x0018_1818);

/// Window alpha (0..255) per mode: off-task is near-opaque and attention-getting,
/// on-task is dimmer so the strip reads as a peripheral cue.
const ALPHA_OFFTASK: u8 = 230;
const ALPHA_ONTASK: u8 = 150;

const BTN_FACE: COLORREF = COLORREF(0x0040_4040);
const BTN_FRAME: COLORREF = COLORREF(0x0090_9090);

const BTN_W: i32 = 64;
const N_BTNS: i32 = 3;

/// A task window's prompt, left→right; the last sits flush against the right edge.
const BTNS_START: [(&str, PromptClick); N_BTNS as usize] = [
    ("Start", PromptClick::Start),
    ("Snooze", PromptClick::Snooze),
    ("Skip", PromptClick::Skip),
];

/// A §6.5 drift check-in: the box asks a question, so it takes an answer.
const BTNS_YESNO: [(&str, PromptClick); N_BTNS as usize] = [
    ("Yes", PromptClick::Yes),
    ("No", PromptClick::No),
    ("Break", PromptClick::Break),
];

/// Where the strip docks on the primary display's vertical edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorPos {
    Top,
    Bottom,
}

/// Strip geometry sourced from `[anchor]` in rules.toml (height + docking edge).
/// Recomputed whenever rules load so a live reload re-sizes/re-docks the next
/// prompt. Width always spans the primary display.
#[derive(Debug, Clone, Copy)]
pub struct AnchorGeom {
    pub height: i32,
    pub pos: AnchorPos,
}

impl AnchorGeom {
    /// Map the parsed `[anchor]` block to concrete geometry. Unknown position
    /// strings fall back to top; height is clamped to at least 1px.
    pub fn from_rules(a: &nudge_core::rules::Anchor) -> Self {
        let pos = match a.position.to_ascii_lowercase().as_str() {
            "bottom" => AnchorPos::Bottom,
            _ => AnchorPos::Top,
        };
        AnchorGeom { height: (a.height_px.max(1)) as i32, pos }
    }
}

/// A user click on the prompt or the §6.5 task list. Drained by the message
/// loop, which maps each to a core `Event`; a click on the strip body is
/// [`PromptClick::Open`], handled as a pure svc-side side effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptClick {
    Start,
    Snooze,
    Skip,
    /// "Yes, still on it" at a drift check-in → `CheckInYes`.
    Yes,
    /// "No" at a drift check-in → `CheckInNo`, which brings up the task list.
    No,
    /// Take a break, from the check-in box or the list → `BreakFor`.
    Break,
    /// Click on the strip body (left of the button cluster): a notification
    /// click-through that launches/focuses nudge-app (P1). Not a core event.
    Open,
}

static REGISTER: Once = Once::new();

/// Process-global click channel (one prompt window at a time; the WndProc is the
/// only sender, the message loop the only receiver).
static CLICKS: OnceLock<(Sender<PromptClick>, Mutex<Receiver<PromptClick>>)> = OnceLock::new();

fn clicks() -> &'static (Sender<PromptClick>, Mutex<Receiver<PromptClick>>) {
    CLICKS.get_or_init(|| {
        let (tx, rx) = channel();
        (tx, Mutex::new(rx))
    })
}

/// Drain one pending button click, if any (mirror of `Tray::poll`).
pub fn poll_click() -> Option<PromptClick> {
    clicks().1.lock().ok()?.try_recv().ok()
}

/// Post a click from another window onto the same queue the message loop drains
/// — the §6.5 task list's rows and its Break button are answers to the same
/// question the strip asked, so they travel the same seam.
pub fn send_click(c: PromptClick) {
    let _ = clicks().0.send(c);
}

pub struct Anchor {
    hwnd: HWND,
}

impl Anchor {
    pub fn create(text: &str, level: Level, mode: Mode, buttons: Buttons, geom: AnchorGeom) -> Self {
        unsafe {
            let hinst = GetModuleHandleW(None).expect("GetModuleHandleW");
            let class = w!("NudgeAnchor");
            REGISTER.call_once(|| {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(anchor_proc),
                    hInstance: hinst.into(),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassW(&wc);
            });
            let width = GetSystemMetrics(SM_CXSCREEN);
            let y = match geom.pos {
                AnchorPos::Top => 0,
                AnchorPos::Bottom => GetSystemMetrics(SM_CYSCREEN) - geom.height,
            };
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                class,
                &HSTRING::from(text),
                WS_POPUP,
                0,
                y,
                width,
                geom.height,
                None,
                None,
                hinst,
                None,
            )
            .expect("anchor window");
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_tag(level, mode, buttons));
            SetLayeredWindowAttributes(hwnd, COLORREF(0), mode_alpha(mode), LWA_ALPHA)
                .expect("alpha");
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Self { hwnd }
        }
    }

    /// Update text, level, mode and/or button set in place, repainting only when
    /// something changed (avoids the destroy/recreate flicker on every escalation
    /// tick). A mode change also re-applies the layered alpha so an off-task
    /// check-in following an on-task prompt reads at full strength.
    pub fn update(&mut self, text: &str, level: Level, mode: Mode, buttons: Buttons) {
        unsafe {
            let mut dirty = false;
            let tag = state_tag(level, mode, buttons);
            let prev = GetWindowLongPtrW(self.hwnd, GWLP_USERDATA);
            if prev != tag {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, tag);
                if mode_bit(prev) != mode_bit(tag) {
                    let _ = SetLayeredWindowAttributes(
                        self.hwnd,
                        COLORREF(0),
                        mode_alpha(mode),
                        LWA_ALPHA,
                    );
                }
                dirty = true;
            }
            if current_text(self.hwnd) != text {
                let _ = SetWindowTextW(self.hwnd, &HSTRING::from(text));
                dirty = true;
            }
            if dirty {
                let _ = InvalidateRect(self.hwnd, None, true);
            }
        }
    }
}

impl Drop for Anchor {
    fn drop(&mut self) {
        // DestroyWindow — paired teardown, never hide.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Pack the paint state (escalation level + notification mode + button set) into
/// the single `GWLP_USERDATA` slot: level in the low byte, mode in bit 8, buttons
/// in bit 9. The WndProc is stateless, so all it needs to render is stored here.
fn state_tag(level: Level, mode: Mode, buttons: Buttons) -> isize {
    let l = match level {
        Level::L0 => 0,
        Level::L1 => 1,
        Level::L2 => 2,
    };
    let m = match mode {
        Mode::OffTask => 0,
        Mode::OnTask => 1,
    };
    let b = match buttons {
        Buttons::StartSnoozeSkip => 0,
        // P1 stub (PLAN-step3 A.5): the §6.4 task-list picker renders as the
        // Yes/No/Break set — No brings up the §6.5 list, which stands in for
        // the real picker until P2 builds it.
        Buttons::YesNoBreak | Buttons::TaskList => 1,
    };
    l | (m << 8) | (b << 9)
}

/// The mode bit extracted from a packed tag (0 = off-task, 1 = on-task).
fn mode_bit(tag: isize) -> isize {
    (tag >> 8) & 1
}

/// The button set a packed tag names — the single place paint and hit-test agree
/// on which buttons the strip is currently carrying.
fn tag_buttons(tag: isize) -> &'static [(&'static str, PromptClick); N_BTNS as usize] {
    if (tag >> 9) & 1 == 1 {
        &BTNS_YESNO
    } else {
        &BTNS_START
    }
}

fn mode_alpha(mode: Mode) -> u8 {
    match mode {
        Mode::OffTask => ALPHA_OFFTASK,
        Mode::OnTask => ALPHA_ONTASK,
    }
}

/// Strip background from a packed tag: on-task is a flat muted charcoal; off-task
/// follows the escalation ladder color (grey → amber → red).
fn strip_bg(tag: isize) -> COLORREF {
    if mode_bit(tag) == 1 {
        return BG_ONTASK;
    }
    match tag & 0xFF {
        1 => BG_L1,
        2 => BG_L2,
        _ => BG_L0,
    }
}

/// Text color from a packed tag: on-task renders muted, off-task at full contrast.
fn strip_fg(tag: isize) -> COLORREF {
    if mode_bit(tag) == 1 {
        FG_MUTED
    } else {
        FG
    }
}

/// Button `i`'s rect in client coordinates. Kept as the single source of layout
/// truth so paint and hit-test never disagree.
fn button_rect(client_width: i32, client_height: i32, i: i32) -> RECT {
    RECT {
        left: client_width - (N_BTNS - i) * BTN_W,
        top: 0,
        right: client_width - (N_BTNS - 1 - i) * BTN_W,
        bottom: client_height,
    }
}

unsafe fn current_text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd);
    let mut buf = vec![0u16; len as usize + 1];
    let n = GetWindowTextW(hwnd, &mut buf);
    String::from_utf16_lossy(&buf[..n as usize])
}

extern "system" fn anchor_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);

            // Strip background + text color from the packed level+mode tag.
            let tag = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            let brush = CreateSolidBrush(strip_bg(tag));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);

            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, strip_fg(tag));

            // Prompt text, left of the button cluster.
            let mut text_rc = RECT {
                left: rc.left + 12,
                right: rc.right - N_BTNS * BTN_W,
                ..rc
            };
            let mut text: Vec<u16> = current_text(hwnd).encode_utf16().collect();
            DrawTextW(hdc, &mut text, &mut text_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE);

            // Buttons.
            let face = CreateSolidBrush(BTN_FACE);
            let frame = CreateSolidBrush(BTN_FRAME);
            for (i, (label, _)) in tag_buttons(tag).iter().enumerate() {
                let mut br = button_rect(rc.right, rc.bottom, i as i32);
                FillRect(hdc, &br, face);
                FrameRect(hdc, &br, frame);
                let mut lbl: Vec<u16> = label.encode_utf16().collect();
                DrawTextW(hdc, &mut lbl, &mut br, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            }
            let _ = DeleteObject(face);
            let _ = DeleteObject(frame);

            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        },
        WM_LBUTTONDOWN => unsafe {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let mut hit = false;
            let tag = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            for (i, (_, click)) in tag_buttons(tag).iter().enumerate() {
                let br = button_rect(rc.right, rc.bottom, i as i32);
                if x >= br.left && x < br.right && y >= br.top && y < br.bottom {
                    let _ = clicks().0.send(*click);
                    hit = true;
                    break;
                }
            }
            // Any click that misses the buttons lands on the strip body — treat
            // it as a notification click-through (open/focus nudge-app).
            if !hit {
                let _ = clicks().0.send(PromptClick::Open);
            }
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
