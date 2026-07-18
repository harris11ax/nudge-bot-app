//! Google integration state: OAuth client config + token cache. All Google
//! network I/O lives in nudge-app; nudge-svc stays offline/zero-poll
//! (GOOGLE-PLAN.md load-bearing constraint #1). `oauth` (10a) handles PKCE
//! consent + token refresh; `calendar` (10b) is the first API surface built
//! on top of it — Gmail (10e) follows the same shape later.

pub mod calendar;
pub mod gmail;
pub mod oauth;

use crate::db::config_dir;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// User-created OAuth client (Google Cloud Console, "Desktop app" type),
/// dropped at `google_client.json`. Absent -> the Connect button surfaces a
/// setup-needed message instead of a token error.
#[derive(Deserialize)]
pub struct ClientConfig {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
}

fn client_config_path() -> PathBuf {
    config_dir().join("google_client.json")
}

pub fn load_client_config() -> Option<ClientConfig> {
    let bytes = std::fs::read(client_config_path()).ok()?;
    parse_client_config(&bytes)
}

/// Accept both the flat `{"client_id","client_secret"}` form and Google Cloud
/// Console's downloaded Desktop-client wrapper `{"installed":{...}}` (or the
/// `{"web":{...}}` variant), so a user can drop the file verbatim without
/// hand-reformatting it. Extra keys (auth_uri, token_uri, …) are ignored.
fn parse_client_config(bytes: &[u8]) -> Option<ClientConfig> {
    #[derive(Deserialize)]
    struct Wrapper {
        installed: Option<ClientConfig>,
        web: Option<ClientConfig>,
    }
    if let Ok(flat) = serde_json::from_slice::<ClientConfig>(bytes) {
        return Some(flat);
    }
    let w: Wrapper = serde_json::from_slice(bytes).ok()?;
    w.installed.or(w.web)
}

/// Cached tokens, DPAPI-sealed on disk — `refresh_token` is long-lived and
/// otherwise sits as plaintext in `%LOCALAPPDATA%`.
#[derive(Serialize, Deserialize, Clone)]
pub struct TokenCache {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds when `access_token` expires.
    pub expires_at: i64,
    /// Space-separated scopes actually granted, so a later phase that needs a
    /// wider scope (e.g. `calendar.events` in 10c) can detect it must re-consent.
    pub scope: String,
}

fn token_cache_path() -> PathBuf {
    config_dir().join("google_tokens.json")
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before 1970")
        .as_secs() as i64
}

impl TokenCache {
    /// 60s skew so a caller never starts a request with a token that expires
    /// mid-flight.
    pub fn is_expired(&self) -> bool {
        now_unix() >= self.expires_at - 60
    }

    pub fn load() -> Option<Self> {
        let sealed = std::fs::read(token_cache_path()).ok()?;
        let plain = dpapi::unprotect(&sealed).ok()?;
        serde_json::from_slice(&plain).ok()
    }

    pub fn save(&self) -> Result<(), String> {
        let plain = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        let sealed = dpapi::protect(&plain)?;
        let path = token_cache_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, sealed).map_err(|e| e.to_string())
    }
}

/// Connect state surfaced to the frontend.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectState {
    /// No `google_client.json` on disk — the user hasn't set up an OAuth client.
    NotConfigured,
    /// `google_client.json` exists but doesn't parse (wrong content, e.g. not
    /// JSON, or a bad encoding). Distinct from `NotConfigured` so the UI can tell
    /// the user their file is malformed rather than missing.
    Misconfigured,
    Disconnected,
    Connected,
}

#[tauri::command]
pub fn google_status() -> ConnectState {
    match std::fs::read(client_config_path()) {
        // Truly absent -> setup not started.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ConnectState::NotConfigured,
        // Present but unreadable (permissions, etc.) -> a config problem, not "missing".
        Err(_) => ConnectState::Misconfigured,
        Ok(bytes) => match parse_client_config(&bytes) {
            None => ConnectState::Misconfigured,
            Some(_) => match TokenCache::load() {
                Some(_) => ConnectState::Connected,
                None => ConnectState::Disconnected,
            },
        },
    }
}

/// Scopes requested at connect time. `calendar.readonly` covers 10b's list
/// calls; `calendar.events` (10c) additionally allows `events.insert`/
/// `events.update` on the primary calendar. A user who connected before 10c
/// only holds the narrower scope — `google_connect` re-runs full consent
/// (`oauth.rs` always sends `prompt=consent`), so reconnecting picks up the
/// wider grant.
/// `gmail.readonly` (10e) is added last; a user connected before 10e holds only
/// the calendar scopes, so `google_connect`'s `prompt=consent` re-consent picks
/// up Gmail when they reconnect.
const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/calendar.readonly",
    "https://www.googleapis.com/auth/calendar.events",
    "https://www.googleapis.com/auth/gmail.readonly",
];

/// Run the PKCE loopback consent flow and cache the resulting tokens. Blocks
/// on the system browser + one redirect; Tauri runs sync commands off the
/// main thread so this doesn't freeze the UI.
#[tauri::command]
pub fn google_connect() -> Result<(), String> {
    let cfg = load_client_config()
        .ok_or_else(|| "no google_client.json — see README for setup".to_string())?;
    let tokens = oauth::run_flow(&cfg, SCOPES)?;
    tokens.save()
}

/// Drop the cached tokens so the next `google_connect` runs full consent from
/// scratch — the fix when a stale token holds narrower scopes than `SCOPES`
/// (e.g. a pre-10e token lacking `gmail.readonly`, which surfaces as a 403 on
/// the first Gmail call). Removing the file is idempotent: an already-absent
/// cache is success, not an error.
#[tauri::command]
pub fn google_disconnect() -> Result<(), String> {
    match std::fs::remove_file(token_cache_path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// A valid access token, refreshing via the cached `refresh_token` if needed.
/// `calendar.rs`/`gmail.rs` (10b+) call this before each API request.
pub fn access_token() -> Result<String, String> {
    let cfg = load_client_config().ok_or("google not configured")?;
    let cache = TokenCache::load().ok_or("google not connected")?;
    if !cache.is_expired() {
        return Ok(cache.access_token);
    }
    let refreshed = oauth::refresh(&cfg, &cache.refresh_token)?;
    refreshed.save()?;
    Ok(refreshed.access_token)
}

#[cfg(windows)]
mod dpapi {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    /// Encrypt for the current Windows user (DPAPI). Machine+user bound —
    /// enough to keep the refresh token off disk as plaintext; not a defense
    /// against another process running as this same user.
    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: plain.len() as u32,
                pbData: plain.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptProtectData(&input, PCWSTR::null(), None, None, None, 0, &mut output)
                .map_err(|e| format!("CryptProtectData: {e}"))?;
            Ok(take_blob(output))
        }
    }

    pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: sealed.len() as u32,
                pbData: sealed.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptUnprotectData(&input, None, None, None, None, 0, &mut output)
                .map_err(|e| format!("CryptUnprotectData: {e}"))?;
            Ok(take_blob(output))
        }
    }

    /// Copy a DPAPI-allocated out-blob and free it with `LocalFree`, per the
    /// CryptProtectData/CryptUnprotectData contract.
    unsafe fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let bytes = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(blob.pbData as *mut _)));
        bytes
    }
}

#[cfg(not(windows))]
mod dpapi {
    pub fn protect(_plain: &[u8]) -> Result<Vec<u8>, String> {
        Err("google token cache requires Windows (DPAPI)".into())
    }
    pub fn unprotect(_sealed: &[u8]) -> Result<Vec<u8>, String> {
        Err("google token cache requires Windows (DPAPI)".into())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_client_config;

    #[test]
    fn flat_form() {
        let c = parse_client_config(br#"{"client_id":"cid","client_secret":"sec"}"#).unwrap();
        assert_eq!(c.client_id, "cid");
        assert_eq!(c.client_secret.as_deref(), Some("sec"));
    }

    #[test]
    fn google_installed_wrapper_with_extra_keys() {
        let raw = br#"{"installed":{"client_id":"cid","project_id":"p",
            "auth_uri":"https://accounts.google.com/o/oauth2/auth",
            "token_uri":"https://oauth2.googleapis.com/token",
            "client_secret":"sec","redirect_uris":["http://localhost"]}}"#;
        let c = parse_client_config(raw).unwrap();
        assert_eq!(c.client_id, "cid");
        assert_eq!(c.client_secret.as_deref(), Some("sec"));
    }

    #[test]
    fn web_wrapper() {
        let c = parse_client_config(br#"{"web":{"client_id":"cid"}}"#).unwrap();
        assert_eq!(c.client_id, "cid");
        assert!(c.client_secret.is_none());
    }

    #[test]
    fn garbage_is_none() {
        assert!(parse_client_config(b"not json").is_none());
        assert!(parse_client_config(br#"{"nope":1}"#).is_none());
    }
}
