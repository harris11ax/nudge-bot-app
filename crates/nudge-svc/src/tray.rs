//! Tray icon + menu via the `tray-icon` crate (tauri/muda). Handles the Win11
//! v4 notify-icon quirks (NIM_SETVERSION, TaskbarCreated re-add, v4 callback
//! decoding) that the hand-rolled version got wrong. Menu clicks arrive on a
//! global channel drained by the message loop via `poll()`.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// The §6.6 Pause submenu durations, label ↔ seconds. "Default" carries `None`
/// and falls back to `[escalation] pause_secs` (the pre-P4 single-item
/// behaviour). "Custom" (step 5) falls back to `[escalation] custom_pause_secs`
/// the same way — no free-text input surface exists in this Win32 app, so the
/// duration is edited in rules.toml and picked up live via Tray > Reload rules.
const PAUSE_CHOICES: [(&str, i64); 5] = [
    ("20 minutes", 20 * 60),
    ("30 minutes", 30 * 60),
    ("45 minutes", 45 * 60),
    ("1 hour", 60 * 60),
    ("1.5 hours", 90 * 60),
];

pub enum TrayCmd {
    Toggle,
    /// Silence everything for this many seconds (§6.6 duration submenu);
    /// `None` = the configured `[escalation] pause_secs`. The rules value lives
    /// in main, so the loop hands the choice up rather than building the core
    /// event itself.
    Pause(Option<i64>),
    /// The submenu's "Custom" item: falls back to `[escalation]
    /// custom_pause_secs` (step 5 — see [`PAUSE_CHOICES`] doc comment).
    PauseCustom,
    /// End a pause/break early.
    Resume,
    Reload,
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    toggle: MenuId,
    /// One id per Pause submenu duration, parallel to [`PAUSE_CHOICES`].
    pause_ids: Vec<MenuId>,
    /// The "Default (rules)" submenu item → `TrayCmd::Pause(None)`.
    pause_default: MenuId,
    /// The "Custom (rules)" submenu item → `TrayCmd::PauseCustom`.
    pause_custom: MenuId,
    resume: MenuId,
    reload: MenuId,
    quit: MenuId,
}

impl Tray {
    /// Build the tray icon on the current (message-loop) thread. Must be called
    /// on the same thread that pumps messages, or callbacks won't be delivered.
    pub fn install() -> Self {
        let toggle = MenuItem::new("Toggle anchor", true, None);
        // Pause/Resume are both always enabled: the state machine ignores a
        // Resume when nothing is paused, so the menu needn't track live state.
        let pause = Submenu::new("Pause", true);
        let mut pause_ids = Vec::with_capacity(PAUSE_CHOICES.len());
        for (label, _) in PAUSE_CHOICES {
            let item = MenuItem::new(label, true, None);
            pause.append(&item).expect("append pause duration");
            pause_ids.push(item.id().clone());
        }
        let default_item = MenuItem::new("Default (rules)", true, None);
        pause.append(&default_item).expect("append pause default");
        let pause_default = default_item.id().clone();
        let custom_item = MenuItem::new("Custom (rules)", true, None);
        pause.append(&custom_item).expect("append pause custom");
        let pause_custom = custom_item.id().clone();
        let resume = MenuItem::new("Resume", true, None);
        let reload = MenuItem::new("Reload rules", true, None);
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        menu.append(&toggle).expect("append toggle");
        menu.append(&PredefinedMenuItem::separator()).expect("append separator");
        menu.append(&pause).expect("append pause");
        menu.append(&resume).expect("append resume");
        menu.append(&PredefinedMenuItem::separator()).expect("append separator");
        menu.append(&reload).expect("append reload");
        menu.append(&PredefinedMenuItem::separator()).expect("append separator");
        menu.append(&quit).expect("append quit");

        let icon = TrayIconBuilder::new()
            .with_tooltip("nudge-bot")
            .with_menu(Box::new(menu))
            .with_icon(default_icon())
            .build()
            .expect("tray icon build");

        Self {
            icon,
            toggle: toggle.id().clone(),
            pause_ids,
            pause_default,
            pause_custom,
            resume: resume.id().clone(),
            reload: reload.id().clone(),
            quit: quit.id().clone(),
        }
    }

    /// Reflect paused state on the icon (§6.6): pause bars while silenced, the
    /// start arrow otherwise. Idempotent — main calls it after every
    /// transition with the current truth.
    pub fn set_paused(&self, paused: bool) {
        let icon = if paused { paused_icon() } else { default_icon() };
        if let Err(e) = self.icon.set_icon(Some(icon)) {
            eprintln!("tray set_icon failed: {e}");
        }
    }

    /// Drain one pending menu event, if any. Returns the mapped command.
    pub fn poll(&self) -> Option<TrayCmd> {
        let ev = MenuEvent::receiver().try_recv().ok()?;
        if let Some(i) = self.pause_ids.iter().position(|id| *id == ev.id) {
            return Some(TrayCmd::Pause(Some(PAUSE_CHOICES[i].1)));
        }
        if ev.id == self.toggle {
            Some(TrayCmd::Toggle)
        } else if ev.id == self.pause_default {
            Some(TrayCmd::Pause(None))
        } else if ev.id == self.pause_custom {
            Some(TrayCmd::PauseCustom)
        } else if ev.id == self.resume {
            Some(TrayCmd::Resume)
        } else if ev.id == self.reload {
            Some(TrayCmd::Reload)
        } else if ev.id == self.quit {
            Some(TrayCmd::Quit)
        } else {
            None
        }
    }
}

/// 32x32 app icon, drawn procedurally (no image-asset pipeline): a rounded
/// blue tile with a white upward arrow — the "start / initiate a task" motif
/// behind the session-8 pivot. Kept in code so the crate stays asset-free.
fn default_icon() -> Icon {
    tile_icon([0x2E, 0x7D, 0xFF], in_up_arrow)
}

/// Paused variant (§6.6): grey tile, white pause bars — glanceably distinct
/// from the blue start arrow while the tray is silenced.
fn paused_icon() -> Icon {
    tile_icon([0x8A, 0x8A, 0x8A], in_pause_bars)
}

/// Rounded tile of `tile` color with a white glyph where `glyph(x, y)` holds.
fn tile_icon(tile: [u8; 3], glyph: fn(i32, i32) -> bool) -> Icon {
    const SIZE: i32 = 32;
    const RADIUS: i32 = 6; // rounded-corner radius, in px
    const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];

    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let px = if !in_rounded_rect(x, y, SIZE, RADIUS) {
                [0, 0, 0, 0] // transparent outside the tile
            } else if glyph(x, y) {
                [WHITE[0], WHITE[1], WHITE[2], 0xFF]
            } else {
                [tile[0], tile[1], tile[2], 0xFF]
            };
            rgba.extend_from_slice(&px);
        }
    }
    Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).expect("icon from_rgba")
}

/// True if (x,y) falls on the white pause glyph: two 5px bars, y 9..=23.
fn in_pause_bars(x: i32, y: i32) -> bool {
    (9..=23).contains(&y) && ((9..=13).contains(&x) || (18..=22).contains(&x))
}

/// True if (x,y) is inside a `size`x`size` square with `radius` rounded corners.
fn in_rounded_rect(x: i32, y: i32, size: i32, radius: i32) -> bool {
    let max = size - 1;
    // Nearest corner-center; only the corner quadrants are tested against the arc.
    let cx = if x < radius {
        radius
    } else if x > max - radius {
        max - radius
    } else {
        x // straight edge column — always inside
    };
    let cy = if y < radius {
        radius
    } else if y > max - radius {
        max - radius
    } else {
        y
    };
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= radius * radius
}

/// True if (x,y) falls on the white up-arrow glyph (triangular head + stem).
fn in_up_arrow(x: i32, y: i32) -> bool {
    const CX: i32 = 16; // arrow centre column
    // Head: apex at y=8 widening to a half-width of 8 at its base (y=16).
    if (8..=16).contains(&y) {
        let half = y - 8;
        if (x - CX).abs() <= half {
            return true;
        }
    }
    // Stem: 6px-wide column running from under the head to y=24.
    if (16..=24).contains(&y) && (x - CX).abs() <= 3 {
        return true;
    }
    false
}
