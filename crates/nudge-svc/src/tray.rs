//! Tray icon + menu via the `tray-icon` crate (tauri/muda). Handles the Win11
//! v4 notify-icon quirks (NIM_SETVERSION, TaskbarCreated re-add, v4 callback
//! decoding) that the hand-rolled version got wrong. Menu clicks arrive on a
//! global channel drained by the message loop via `poll()`.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub enum TrayCmd {
    Toggle,
    Reload,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    toggle: MenuId,
    reload: MenuId,
    quit: MenuId,
}

impl Tray {
    /// Build the tray icon on the current (message-loop) thread. Must be called
    /// on the same thread that pumps messages, or callbacks won't be delivered.
    pub fn install() -> Self {
        let toggle = MenuItem::new("Toggle anchor", true, None);
        let reload = MenuItem::new("Reload rules", true, None);
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        menu.append(&toggle).expect("append toggle");
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
            _icon: icon,
            toggle: toggle.id().clone(),
            reload: reload.id().clone(),
            quit: quit.id().clone(),
        }
    }

    /// Drain one pending menu event, if any. Returns the mapped command.
    pub fn poll(&self) -> Option<TrayCmd> {
        let ev = MenuEvent::receiver().try_recv().ok()?;
        if ev.id == self.toggle {
            Some(TrayCmd::Toggle)
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
    const SIZE: i32 = 32;
    const RADIUS: i32 = 6; // rounded-corner radius, in px
    const BLUE: [u8; 3] = [0x2E, 0x7D, 0xFF];
    const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];

    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let px = if !in_rounded_rect(x, y, SIZE, RADIUS) {
                [0, 0, 0, 0] // transparent outside the tile
            } else if in_up_arrow(x, y) {
                [WHITE[0], WHITE[1], WHITE[2], 0xFF]
            } else {
                [BLUE[0], BLUE[1], BLUE[2], 0xFF]
            };
            rgba.extend_from_slice(&px);
        }
    }
    Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).expect("icon from_rgba")
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
