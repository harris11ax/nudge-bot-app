//! Google integration state: OAuth client config + token cache. All Google
//! network I/O lives in nudge-app; nudge-svc stays offline/zero-poll
//! (GOOGLE-PLAN.md load-bearing constraint #1). `oauth` (10a) handles PKCE
//! consent + token refresh; `calendar` (10b) is the first API surface built
//! on top of it — Gmail (10e) follows the same shape later.

pub mod calendar;
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
    serde_json::from_slice(&bytes).ok()
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
    NotConfigured,
    Disconnected,
    Connected,
}

#[tauri::command]
pub fn google_status() -> ConnectState {
    if load_client_config().is_none() {
        return ConnectState::NotConfigured;
    }
    match TokenCache::load() {
        Some(_) => ConnectState::Connected,
        None => ConnectState::Disconnected,
    }
}

/// Scopes requested by the Calendar-read work that immediately follows this
/// session (10b) — requesting it now avoids a second consent round.
const SCOPES: &[&str] = &["https://www.googleapis.com/auth/calendar.readonly"];

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
