//! The Tier-B P2 classification screen: after a check-in is affirmed, each tool
//! seen since the last check-in is routed to the picked task's tool list, the
//! global not-tool list, or a task-scoped ignore (§6.4/§6.5).
//!
//! Same window discipline as [`crate::tasklist`]: layered, topmost,
//! `WS_EX_NOACTIVATE`, destroyed (not hidden) on drop; paint and hit-test share
//! `row_rect`/`btn_rect` as the single layout truth (the invariant that made
//! Step 2b's buttons work).
//!
//! A row click marks the row and posts `PromptClick::ClassifyRow(i, choice)`
//! onto the same channel the strip uses; when the last unrouted row is chosen
//! (or Done is clicked) a `PromptClick::ClassifyDone` follows. The row-index →
//! app-name resolution happens at drain time via [`app_at`], which keeps
//! `PromptClick` `Copy`.

use std::sync::{Mutex, Once, OnceLock};

use nudge_core::state::ClassifyChoice;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect,
    InvalidateRect, SetBkMode, SetTextColor, DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER,
    PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetSystemMetrics,
    RegisterClassW, SetLayeredWindowAttributes, ShowWindow, LWA_ALPHA, SM_CXSCREEN, SM_CYSCREEN,
    SW_SHOWNOACTIVATE, WM_LBUTTONDOWN, WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::overlay::{send_click, PromptClick};

const WIDTH: i32 = 560;
const HEADER_H: i32 = 34;
const ROW_H: i32 = 32;
const FOOTER_H: i32 = 38;
const PAD: i32 = 12;
const ALPHA: u8 = 235;

const BTN_W: i32 = 72;
const BTN_GAP: i32 = 6;
const N_CHOICES: usize = 3;

const BG: COLORREF = COLORREF(0x0020_2020);
const FG: COLORREF = COLORREF(0x00F0_F0F0);
const FG_DIM: COLORREF = COLORREF(0x00A0_A0A0);
const ROW_FACE: COLORREF = COLORREF(0x0030_3030);
const BTN_FACE: COLORREF = COLORREF(0x0040_4040);
const BTN_FRAME: COLORREF = COLORREF(0x0090_9090);
/// A chosen button's face: visibly "pressed" so the user sees the routing land.
const BTN_CHOSEN: COLORREF = COLORREF(0x0060_A0_60);

const HEADER: &str = "Tools since your last check-in — where do they go?";
const DONE_LABEL: &str = "Done";

/// Label + choice per column, left→right in each row.
const CHOICES: [(&str, ClassifyChoice); N_CHOICES] = [
    ("Tool", ClassifyChoice::Tool),
    ("Not a tool", ClassifyChoice::NotTool),
    ("Ignore", ClassifyChoice::Ignore),
];

static REGISTER: Once = Once::new();

/// The rows on screen: each accumulated tool and the choice made so far. One
/// classify screen exists at a time — the WndProc is stateless and reads here.
static ROWS: OnceLock<Mutex<Vec<(String, Option<ClassifyChoice>)>>> = OnceLock::new();

fn rows() -> &'static Mutex<Vec<(String, Option<ClassifyChoice>)>> {
    ROWS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Resolve a row index back to its app name — the message loop calls this while
/// draining a `ClassifyRow` click. `None` if the screen was torn down (or the
/// index is stale), in which case the click is dropped, not misrouted.
pub fn app_at(i: usize) -> Option<String> {
    rows().lock().ok()?.get(i).map(|(app, _)| app.clone())
}

pub struct Classify {
    hwnd: HWND,
}

impl Classify {
    /// Put the tool router on screen, centred. Height follows the tool count;
    /// the accumulator is naturally small (one entry per distinct foreground app
    /// per check-in window), so no scroll for P2.
    pub fn create(tools: &[String]) -> Self {
        if let Ok(mut g) = rows().lock() {
            *g = tools.iter().map(|t| (t.clone(), None)).collect();
        }
        unsafe {
            let hinst = GetModuleHandleW(None).expect("GetModuleHandleW");
            let class = w!("NudgeClassify");
            REGISTER.call_once(|| {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(classify_proc),
                    hInstance: hinst.into(),
                    lpszClassName: class,
                    ..Default::default()
                };
                RegisterClassW(&wc);
            });
            let height = HEADER_H + ROW_H * tools.len() as i32 + FOOTER_H;
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
            .expect("classify window");
            SetLayeredWindowAttributes(hwnd, COLORREF(0), ALPHA, LWA_ALPHA).expect("alpha");
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Self { hwnd }
        }
    }
}

impl Drop for Classify {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
        if let Ok(mut g) = rows().lock() {
            g.clear();
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

/// Choice button `c` (0..3, left→right) inside row `i`'s rect.
fn btn_rect(i: i32, c: i32) -> RECT {
    let rr = row_rect(i);
    let right = rr.right - 4 - (N_CHOICES as i32 - 1 - c) * (BTN_W + BTN_GAP);
    RECT { left: right - BTN_W, top: rr.top + 3, right, bottom: rr.bottom - 3 }
}

/// The Done button's rect, bottom-right.
fn done_rect(client_height: i32) -> RECT {
    RECT {
        left: WIDTH - PAD - 90,
        top: client_height - FOOTER_H + 4,
        right: WIDTH - PAD,
        bottom: client_height - 8,
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

extern "system" fn classify_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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

            SetTextColor(hdc, FG_DIM);
            let mut hrc = RECT { left: PAD, top: 0, right: WIDTH - PAD, bottom: HEADER_H };
            draw(hdc, HEADER, &mut hrc, (DT_LEFT | DT_VCENTER | DT_SINGLELINE).0);

            let face = CreateSolidBrush(ROW_FACE);
            let btn_face = CreateSolidBrush(BTN_FACE);
            let btn_chosen = CreateSolidBrush(BTN_CHOSEN);
            let btn_frame = CreateSolidBrush(BTN_FRAME);
            for (i, (app, chosen)) in guard.iter().enumerate() {
                let rr = row_rect(i as i32);
                FillRect(hdc, &rr, face);

                SetTextColor(hdc, FG);
                let text_right = btn_rect(i as i32, 0).left - 8;
                let mut trc = RECT { left: rr.left + 8, right: text_right, ..rr };
                draw(hdc, app, &mut trc, (DT_LEFT | DT_VCENTER | DT_SINGLELINE).0);

                for (c, (label, choice)) in CHOICES.iter().enumerate() {
                    let mut br = btn_rect(i as i32, c as i32);
                    let picked = *chosen == Some(*choice);
                    FillRect(hdc, &br, if picked { btn_chosen } else { btn_face });
                    FrameRect(hdc, &br, btn_frame);
                    SetTextColor(hdc, FG);
                    draw(hdc, label, &mut br, (DT_CENTER | DT_VCENTER | DT_SINGLELINE).0);
                }
            }
            let _ = DeleteObject(face);

            let mut dr = done_rect(rc.bottom);
            FillRect(hdc, &dr, btn_face);
            FrameRect(hdc, &dr, btn_frame);
            SetTextColor(hdc, FG);
            draw(hdc, DONE_LABEL, &mut dr, (DT_CENTER | DT_VCENTER | DT_SINGLELINE).0);
            let _ = DeleteObject(btn_face);
            let _ = DeleteObject(btn_chosen);
            let _ = DeleteObject(btn_frame);

            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        },
        WM_LBUTTONDOWN => unsafe {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);

            let dr = done_rect(rc.bottom);
            if x >= dr.left && x < dr.right && y >= dr.top && y < dr.bottom {
                send_click(PromptClick::ClassifyDone);
                return LRESULT(0);
            }

            let Ok(mut guard) = rows().lock() else { return LRESULT(0) };
            let n = guard.len();
            'hit: for i in 0..n as i32 {
                for (c, (_, choice)) in CHOICES.iter().enumerate() {
                    let br = btn_rect(i, c as i32);
                    if x >= br.left && x < br.right && y >= br.top && y < br.bottom {
                        guard[i as usize].1 = Some(*choice);
                        send_click(PromptClick::ClassifyRow(i as usize, *choice));
                        // Last unrouted row just got its choice → the screen's
                        // job is done; the core tears it down on ClassifyDone.
                        if guard.iter().all(|(_, ch)| ch.is_some()) {
                            send_click(PromptClick::ClassifyDone);
                        }
                        drop(guard);
                        let _ = InvalidateRect(hwnd, None, true);
                        break 'hit;
                    }
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

    // Paint and hit-test share the same rects: rows are disjoint and each row's
    // three buttons are disjoint, ordered, and inside the row.
    #[test]
    fn rects_are_disjoint_and_nested() {
        for i in 0..8 {
            assert!(row_rect(i).bottom <= row_rect(i + 1).top);
            let rr = row_rect(i);
            for c in 0..N_CHOICES as i32 {
                let br = btn_rect(i, c);
                assert!(br.left >= rr.left && br.right <= rr.right);
                assert!(br.top >= rr.top && br.bottom <= rr.bottom);
                if c > 0 {
                    assert!(btn_rect(i, c - 1).right <= br.left);
                }
            }
        }
    }

    // The row store resolves indices to app names and drops stale lookups.
    #[test]
    fn app_at_resolves_and_bounds_checks() {
        if let Ok(mut g) = rows().lock() {
            *g = vec![("code.exe".into(), None), ("game.exe".into(), None)];
        }
        assert_eq!(app_at(0).as_deref(), Some("code.exe"));
        assert_eq!(app_at(1).as_deref(), Some("game.exe"));
        assert_eq!(app_at(2), None);
        if let Ok(mut g) = rows().lock() {
            g.clear();
        }
        assert_eq!(app_at(0), None);
    }
}
