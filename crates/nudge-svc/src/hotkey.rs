//! RegisterHotKey toggle — hardware-interlock stand-in. The `Trigger` trait
//! is the seam where an NFC/BLE trigger slots in later (interface only;
//! persistent radio listeners are out of budget).

use nudge_core::rules::Hotkey;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    MOD_SHIFT, MOD_WIN,
};

const HOTKEY_ID: i32 = 1;

pub trait Trigger {
    /// Event surfaces in the message loop as WM_HOTKEY -> Event::HotkeyToggle.
    fn id(&self) -> i32;
}

pub struct Toggle {
    id: i32,
}

impl Toggle {
    pub fn register(cfg: &Hotkey) -> Self {
        let mut mods = MOD_NOREPEAT;
        for m in &cfg.modifiers {
            mods |= match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => MOD_CONTROL,
                "alt" => MOD_ALT,
                "shift" => MOD_SHIFT,
                "win" => MOD_WIN,
                other => panic!("unknown hotkey modifier '{other}'"),
            };
        }
        let vk = parse_key(&cfg.key);
        // hwnd = None: WM_HOTKEY is posted to the thread queue, where the
        // main message loop picks it up.
        unsafe {
            RegisterHotKey(None, HOTKEY_ID, HOT_KEY_MODIFIERS(mods.0), vk)
                .expect("RegisterHotKey (in use by another app?)");
        }
        Self { id: HOTKEY_ID }
    }
}

/// Virtual-key code from config: single ASCII letter/digit, or "F1".."F24".
fn parse_key(key: &str) -> u32 {
    let up = key.to_ascii_uppercase();
    if let Some(n) = up.strip_prefix('F').and_then(|r| r.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return 0x6F + n; // VK_F1 = 0x70
        }
    }
    let bytes = up.as_bytes();
    if bytes.len() == 1 && bytes[0].is_ascii_alphanumeric() {
        return bytes[0] as u32; // VK codes for 0-9/A-Z match ASCII
    }
    panic!("unsupported hotkey key '{key}'");
}

impl Trigger for Toggle {
    fn id(&self) -> i32 {
        self.id
    }
}

impl Drop for Toggle {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterHotKey(None, self.id);
        }
    }
}
