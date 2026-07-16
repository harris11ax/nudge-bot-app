//! The §6.5 task list: the window that comes up when the user answers No to a
//! drift check-in — "not that? then here's what's actually due."
//!
//! Same window discipline as [`crate::overlay`]: layered, topmost, and
//! `WS_EX_NOACTIVATE` so bringing the list to the user never steals the caret out
//! from under them; destroyed (not hidden) on drop.
//!
//! This module paints and hit-tests. It does not decide what is in the list, in
//! what order, or in what colour — those are `task_window::display_list`'s, and
//! arrive here as finished [`Row`]s (UI-PLAN §6.9: the one owner). The only
//! reason a `StyleClass` is matched on below is to name the paint for a class the
//! core already chose.

use std::sync::{Mutex, Once, OnceLock};

use nudge_core::task_window::{Row, StyleClass};
use nudge_core::UnixTime;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect,
    SetBkMode, SetTextColor, DT_CENTER, DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, PAINTSTRUCT,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics,
    RegisterClassW, SetLayeredWindowAttributes, ShowWindow, LWA_ALPHA, SM_CXSCREEN, SM_CYSCREEN,
    SW_SHOWNOACTIVATE, WM_LBUTTONDOWN, WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::overlay::{send_click, PromptClick};

const WIDTH: i32 = 520;
const HEADER_H: i32 = 34;
const ROW_H: i32 = 30;
const FOOTER_H: i32 = 38;
const PAD: i32 = 12;
const ALPHA: u8 = 235;

const BG: COLORREF = COLORREF(0x0020_2020);
const FG: COLORREF = COLORREF(0x00F0_F0F0);
const FG_DIM: COLORREF = COLORREF(0x00A0_A0A0);
const ROW_FACE: COLORREF = COLORREF(0x0030_3030);
const BTN_FACE: COLORREF = COLORREF(0x0040_4040);
const BTN_FRAME: COLORREF = COLORREF(0x0090_9090);

/// §6.9 outline colours. `NotStartedUrgent` is the red one the spec calls for at
/// <24 h; the bands run cool→warm as a task fills up its estimate.
const OUTLINE_NOT_STARTED: COLORREF = COLORREF(0x00D0_D0D0);
const OUTLINE_URGENT: COLORREF = COLORREF(0x0020_20E0);
const OUTLINE_BANDS: [COLORREF; 3] = [
    COLORREF(0x0070_7070),
    COLORREF(0x0060_C0_60),
    COLORREF(0x0040_A0_E0),
];

const HEADER: &str = "What's actually due";
const BREAK_LABEL: &str = "Take a break";

static REGISTER: Once = Once::new();

/// The rows currently on screen, plus the instant they were computed for (so
/// "due in" reads relative to the list, not to whenever a repaint happens). One
/// list exists at a time — the WndProc is stateless and reads from here.
static ROWS: OnceLock<Mutex<(Vec<Row>, UnixTime)>> = OnceLock::new();

fn rows() -> &'static Mutex<(Vec<Row>, UnixTime)> {
    ROWS.get_or_init(|| Mutex::new((Vec::new(), 0)))
}

pub struct TaskList {
    hwnd: HWND,
}

impl TaskList {
    /// Put `rows` on screen, centred. Height follows the row count — the list is
    /// already capped at `WindowCfg::max_rows` (12) by `display_list`, so this
    /// cannot grow without bound.
    pub fn create(new_rows: &[Row], now: UnixTime) -> Self {
        if let Ok(mut g) = rows().lock() {
            *g = (new_rows.to_vec(), now);
        }
        unsafe {
            let hinst = GetModuleHandleW(None).expect("GetModuleHandleW");
            let class = w!("NudgeTaskList");
            REGISTER.call_once(|| {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(list_proc),
                    hInstance: hinst.into(),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassW(&wc);
            });
            let height = HEADER_H + ROW_H * new_rows.len() as i32 + FOOTER_H;
            let x = (GetSystemMetrics(SM_CXSCREEN) - WIDTH) / 2;
            let y = (GetSystemMetrics(SM_CYSCREEN) - height) / 2;
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                class,
                w!("nudge"),
                WS_POPUP,
                x,
                y,
                WIDTH,
                height,
                None,
                None,
                hinst,
                None,
            )
            .expect("task list window");
            SetLayeredWindowAttributes(hwnd, COLORREF(0), ALPHA, LWA_ALPHA).expect("alpha");
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Self { hwnd }
        }
    }
}

impl Drop for TaskList {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
        if let Ok(mut g) = rows().lock() {
            g.0.clear();
        }
    }
}

/// Row `i`'s rect — the single layout truth shared by paint and hit-test.
fn row_rect(i: i32) -> RECT {
    RECT {
        left: PAD,
        top: HEADER_H + i * ROW_H,
        right: WIDTH - PAD,
        bottom: HEADER_H + (i + 1) * ROW_H - 4,
    }
}

/// The Take-a-break button's rect, bottom-right.
fn break_rect(client_height: i32) -> RECT {
    RECT {
        left: WIDTH - PAD - 110,
        top: client_height - FOOTER_H + 4,
        right: WIDTH - PAD,
        bottom: client_height - 8,
    }
}

/// The outline a row's [`StyleClass`] calls for. Bands beyond the palette clamp
/// to its last colour rather than panicking — core owns how many bands exist and
/// the user can add more from the Style settings.
fn outline(style: &StyleClass) -> COLORREF {
    match style {
        StyleClass::NotStarted => OUTLINE_NOT_STARTED,
        StyleClass::NotStartedUrgent => OUTLINE_URGENT,
        StyleClass::Band(n) => OUTLINE_BANDS[(*n as usize).min(OUTLINE_BANDS.len() - 1)],
    }
}

/// "due in 3h" / "due in 20m" / "overdue" — a coarse relative deadline, which is
/// all this list is for. No date maths: the row already carries an absolute time.
fn due_text(deadline: Option<UnixTime>, now: UnixTime) -> String {
    let Some(dl) = deadline else { return String::new() };
    let secs = dl - now;
    if secs < 0 {
        return "overdue".into();
    }
    match secs {
        s if s < 3600 => format!("due in {}m", s / 60),
        s if s < 48 * 3600 => format!("due in {}h", s / 3600),
        s => format!("due in {}d", s / (24 * 3600)),
    }
}

/// Progress suffix for a row that has time on it: "45/120m".
fn progress_text(r: &Row) -> String {
    match (r.logged, r.estimate) {
        (0, _) => String::new(),
        (l, Some(e)) => format!("  {l}/{e}m"),
        (l, None) => format!("  {l}m"),
    }
}

fn draw(hdc: windows::Win32::Graphics::Gdi::HDC, text: &str, rc: &mut RECT, flags: u32) {
    let mut buf: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        DrawTextW(
            hdc,
            &mut buf,
            rc,
            windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(flags),
        );
    }
}

extern "system" fn list_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);

            let bg = CreateSolidBrush(BG);
            FillRect(hdc, &rc, bg);
            let _ = DeleteObject(bg);
            SetBkMode(hdc, TRANSPARENT);

            let Ok(guard) = rows().lock() else {
                let _ = EndPaint(hwnd, &ps);
                return LRESULT(0);
            };
            let (list, now) = &*guard;

            SetTextColor(hdc, FG_DIM);
            let mut hrc = RECT { left: PAD, top: 0, right: WIDTH - PAD, bottom: HEADER_H };
            draw(hdc, HEADER, &mut hrc, (DT_LEFT | DT_VCENTER | DT_SINGLELINE).0);

            let face = CreateSolidBrush(ROW_FACE);
            for (i, r) in list.iter().enumerate() {
                let rr = row_rect(i as i32);
                FillRect(hdc, &rr, face);
                let frame = CreateSolidBrush(outline(&r.style));
                FrameRect(hdc, &rr, frame);
                let _ = DeleteObject(frame);

                SetTextColor(hdc, FG);
                let mut trc = RECT { left: rr.left + 8, right: rr.right - 100, ..rr };
                draw(
                    hdc,
                    &format!("{}{}", r.title, progress_text(r)),
                    &mut trc,
                    (DT_LEFT | DT_VCENTER | DT_SINGLELINE).0,
                );
                SetTextColor(hdc, FG_DIM);
                let mut drc = RECT { left: rr.right - 100, right: rr.right - 8, ..rr };
                draw(
                    hdc,
                    &due_text(r.deadline, *now),
                    &mut drc,
                    (DT_RIGHT | DT_VCENTER | DT_SINGLELINE).0,
                );
            }
            let _ = DeleteObject(face);

            let btn_face = CreateSolidBrush(BTN_FACE);
            let btn_frame = CreateSolidBrush(BTN_FRAME);
            let mut br = break_rect(rc.bottom);
            FillRect(hdc, &br, btn_face);
            FrameRect(hdc, &br, btn_frame);
            SetTextColor(hdc, FG);
            draw(hdc, BREAK_LABEL, &mut br, (DT_CENTER | DT_VCENTER | DT_SINGLELINE).0);
            let _ = DeleteObject(btn_face);
            let _ = DeleteObject(btn_frame);

            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        },
        WM_LBUTTONDOWN => unsafe {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);

            let br = break_rect(rc.bottom);
            if x >= br.left && x < br.right && y >= br.top && y < br.bottom {
                send_click(PromptClick::Break);
                return LRESULT(0);
            }
            // A click on a row means "right, I'm on THAT" — the row's id rides
            // along so an OnTask picker can route classification to the picked
            // task. From the §6.5 Choosing list it still resolves like Start
            // (switching the live window to the picked task is Tier-C, P4).
            let ids: Vec<i64> = rows()
                .lock()
                .map(|g| g.0.iter().map(|r| r.task_id).collect())
                .unwrap_or_default();
            for (i, id) in ids.iter().enumerate() {
                let rr = row_rect(i as i32);
                if x >= rr.left && x < rr.right && y >= rr.top && y < rr.bottom {
                    send_click(PromptClick::PickTask(*id));
                    break;
                }
            }
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(style: StyleClass, logged: u32, estimate: Option<u32>) -> Row {
        Row { task_id: 1, title: "t".into(), deadline: None, logged, estimate, style }
    }

    #[test]
    fn due_text_reads_coarsely_and_calls_out_overdue() {
        assert_eq!(due_text(None, 1000), "");
        assert_eq!(due_text(Some(900), 1000), "overdue");
        assert_eq!(due_text(Some(1000 + 20 * 60), 1000), "due in 20m");
        assert_eq!(due_text(Some(1000 + 3 * 3600), 1000), "due in 3h");
        assert_eq!(due_text(Some(1000 + 72 * 3600), 1000), "due in 3d");
        // The 168 h horizon's far edge still reads as days, not hours.
        assert_eq!(due_text(Some(1000 + 168 * 3600), 1000), "due in 7d");
    }

    #[test]
    fn progress_text_stays_empty_until_there_is_progress() {
        assert_eq!(progress_text(&row(StyleClass::NotStarted, 0, Some(120))), "");
        assert_eq!(progress_text(&row(StyleClass::Band(0), 45, Some(120))), "  45/120m");
        assert_eq!(progress_text(&row(StyleClass::Band(0), 45, None)), "  45m");
    }

    // A band core invented beyond this palette clamps instead of panicking — the
    // band count is the user's to set from the Style settings, not ours.
    #[test]
    fn outline_clamps_unknown_bands() {
        assert_eq!(outline(&StyleClass::NotStartedUrgent).0, OUTLINE_URGENT.0);
        assert_eq!(outline(&StyleClass::Band(0)).0, OUTLINE_BANDS[0].0);
        assert_eq!(outline(&StyleClass::Band(99)).0, OUTLINE_BANDS[2].0);
    }

    // Paint and hit-test share `row_rect`, so rows never overlap and the click
    // that lands on row i is the row painted at i.
    #[test]
    fn row_rects_are_disjoint_and_ordered() {
        for i in 0..11 {
            assert!(row_rect(i).bottom <= row_rect(i + 1).top);
        }
        assert_eq!(row_rect(0).top, HEADER_H);
    }
}
