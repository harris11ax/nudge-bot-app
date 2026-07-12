//! OAuth 2.0 PKCE loopback flow (RFC 8252) against Google's endpoints.
//! Blocking `ureq` for the two HTTP calls, matching the workspace's
//! no-async convention (see nudge-draft/src/anthropic.rs). Randomness and
//! SHA-256 (PKCE `S256` challenge) come from Windows CNG (`bcrypt.dll`)
//! rather than pulling in a `rand`/`sha2` crate.

use super::{ClientConfig, TokenCache};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
/// How long to wait on the loopback listener before giving up on a consent
/// flow the user apparently abandoned.
const CONSENT_TIMEOUT: Duration = Duration::from_secs(300);

/// Run the full consent flow for `scopes`: bind a one-shot loopback listener,
/// open the system browser to Google's consent screen, block for the single
/// redirect, then exchange the code for tokens. The caller (a Tauri sync
/// command) already runs off the main thread, so blocking here is fine.
pub fn run_flow(cfg: &ClientConfig, scopes: &[&str]) -> Result<TokenCache, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind loopback: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("loopback addr: {e}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let verifier = random_verifier();
    let challenge = base64url(&sha256(verifier.as_bytes()));
    let state = random_verifier(); // reuses the verifier generator; just needs to be unguessable

    let auth_url = build_auth_url(cfg, &redirect_uri, scopes, &challenge, &state);
    open_browser(&auth_url)?;

    let code = await_redirect(listener, &state)?;
    exchange_code(cfg, &code, &verifier, &redirect_uri)
}

/// Refresh an expired access token using the stored `refresh_token`.
pub fn refresh(cfg: &ClientConfig, refresh_token: &str) -> Result<TokenCache, String> {
    let mut form = vec![
        ("client_id", cfg.client_id.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    if let Some(secret) = &cfg.client_secret {
        form.push(("client_secret", secret.as_str()));
    }
    let resp = post_form(TOKEN_ENDPOINT, &form)?;
    // A refresh response omits refresh_token when Google hasn't rotated it;
    // keep the one we already have in that case.
    token_cache_from_response(resp, Some(refresh_token.to_string()))
}

fn exchange_code(
    cfg: &ClientConfig,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenCache, String> {
    let mut form = vec![
        ("client_id", cfg.client_id.as_str()),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];
    if let Some(secret) = &cfg.client_secret {
        form.push(("client_secret", secret.as_str()));
    }
    let resp = post_form(TOKEN_ENDPOINT, &form)?;
    token_cache_from_response(resp, None)
}

fn build_auth_url(
    cfg: &ClientConfig,
    redirect_uri: &str,
    scopes: &[&str],
    challenge: &str,
    state: &str,
) -> String {
    let scope = scopes.join(" ");
    format!(
        "{AUTH_ENDPOINT}?client_id={}&redirect_uri={}&response_type=code&scope={}\
         &code_challenge={}&code_challenge_method=S256&state={}\
         &access_type=offline&prompt=consent",
        urlenc(&cfg.client_id),
        urlenc(redirect_uri),
        urlenc(&scope),
        urlenc(challenge),
        urlenc(state),
    )
}

/// Open `url` in the user's default browser. `cmd /C start` is the standard
/// dependency-free way to do this on Windows (avoids pulling in the `open`
/// crate for one call).
#[cfg(windows)]
fn open_browser(url: &str) -> Result<(), String> {
    std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("open browser: {e}"))
}

#[cfg(not(windows))]
fn open_browser(_url: &str) -> Result<(), String> {
    Err("google connect requires Windows".into())
}

/// Block for the single OAuth redirect, verifying `state` and returning the
/// authorization `code`. Non-blocking-poll loop (no async runtime) bounded by
/// [`CONSENT_TIMEOUT`] so an abandoned browser tab can't hang the command.
fn await_redirect(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("loopback nonblocking: {e}"))?;
    let deadline = Instant::now() + CONSENT_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return handle_redirect(stream, expected_state),
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("timed out waiting for Google sign-in".into());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("loopback accept: {e}")),
        }
    }
}

fn handle_redirect(mut stream: TcpStream, expected_state: &str) -> Result<String, String> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut buf = [0u8; 4096];
    let n = stream
        .read(&mut buf)
        .map_err(|e| format!("read redirect: {e}"))?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let request_line = request.lines().next().unwrap_or("");
    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    let query = path.splitn(2, '?').nth(1).unwrap_or("");
    let params = parse_query(query);

    let ok = params.get("error").is_none() && params.contains_key("code");
    let body = if ok {
        "<html><body>Signed in &mdash; you can close this window and return to nudge-bot.</body></html>"
    } else {
        "<html><body>Sign-in was cancelled. You can close this window.</body></html>"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();

    if let Some(err) = params.get("error") {
        return Err(format!("google denied consent: {err}"));
    }
    let code = params.get("code").cloned().ok_or("no code in redirect")?;
    if params.get("state").map(String::as_str) != Some(expected_state) {
        return Err("state mismatch on OAuth redirect — aborting".into());
    }
    Ok(code)
}

fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|kv| !kv.is_empty())
        .filter_map(|kv| {
            let mut it = kv.splitn(2, '=');
            let k = urldecode(it.next()?);
            let v = urldecode(it.next().unwrap_or(""));
            Some((k, v))
        })
        .collect()
}

fn post_form(url: &str, form: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(15))
        .build();
    let resp = agent
        .post(url)
        .send_form(form)
        .map_err(|e| format!("token request failed: {e}"))?;
    resp.into_json()
        .map_err(|e| format!("bad token response: {e}"))
}

fn token_cache_from_response(
    v: serde_json::Value,
    fallback_refresh: Option<String>,
) -> Result<TokenCache, String> {
    let access_token = v
        .get("access_token")
        .and_then(|x| x.as_str())
        .ok_or("token response missing access_token")?
        .to_string();
    let expires_in = v.get("expires_in").and_then(|x| x.as_i64()).unwrap_or(3600);
    let scope = v
        .get("scope")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let refresh_token = v
        .get("refresh_token")
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .or(fallback_refresh)
        .ok_or("token response missing refresh_token — retry Connect (needs fresh consent)")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs() as i64;
    Ok(TokenCache {
        access_token,
        refresh_token,
        expires_at: now + expires_in,
        scope,
    })
}

// --- PKCE + percent-encoding helpers (pure, unit-tested) ---

fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Base64url, no padding (RFC 4648 §5) — used for both the PKCE verifier and
/// the S256 challenge.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = (b0 as u32) << 16 | (b1 as u32) << 8 | b2 as u32;
        out.push(ALPHABET[(n >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(n >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 0x3f] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 0x3f] as char);
        }
    }
    out
}

/// A 43-character PKCE verifier: 32 random bytes, base64url-encoded — right
/// at RFC 7636's minimum length, using only its required alphabet.
fn random_verifier() -> String {
    base64url(&random_bytes(32))
}

#[cfg(windows)]
fn random_bytes(n: usize) -> Vec<u8> {
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    let mut buf = vec![0u8; n];
    unsafe {
        BCryptGenRandom(None, &mut buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
            .ok()
            .expect("BCryptGenRandom");
    }
    buf
}

#[cfg(not(windows))]
fn random_bytes(n: usize) -> Vec<u8> {
    vec![0u8; n] // unreachable off Windows: run_flow's open_browser errors first
}

#[cfg(windows)]
fn sha256(data: &[u8]) -> [u8; 32] {
    use windows::Win32::Security::Cryptography::{
        BCryptCloseAlgorithmProvider, BCryptHash, BCryptOpenAlgorithmProvider, BCRYPT_ALG_HANDLE,
        BCRYPT_SHA256_ALGORITHM,
    };
    unsafe {
        let mut halg = BCRYPT_ALG_HANDLE(std::ptr::null_mut());
        BCryptOpenAlgorithmProvider(&mut halg, BCRYPT_SHA256_ALGORITHM, None, Default::default())
            .ok()
            .expect("open SHA256 provider");
        let mut out = [0u8; 32];
        BCryptHash(halg, None, data, &mut out)
            .ok()
            .expect("BCryptHash");
        let _ = BCryptCloseAlgorithmProvider(halg, 0);
        out
    }
}

#[cfg(not(windows))]
fn sha256(_data: &[u8]) -> [u8; 32] {
    [0u8; 32]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlenc_escapes_reserved_chars() {
        assert_eq!(urlenc("a b/c"), "a%20b%2Fc");
        assert_eq!(urlenc("abc-._~XYZ09"), "abc-._~XYZ09");
    }

    #[test]
    fn urldecode_roundtrips_urlenc() {
        let s = "hello world/?=&foo";
        assert_eq!(urldecode(&urlenc(s)), s);
    }

    #[cfg(windows)]
    #[test]
    fn pkce_challenge_matches_rfc7636_test_vector() {
        // RFC 7636 Appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = base64url(&sha256(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn parse_query_extracts_code_and_state() {
        let params = parse_query("code=abc123&state=xyz&scope=a%20b");
        assert_eq!(params.get("code").map(String::as_str), Some("abc123"));
        assert_eq!(params.get("state").map(String::as_str), Some("xyz"));
        assert_eq!(params.get("scope").map(String::as_str), Some("a b"));
    }

    #[test]
    fn build_auth_url_carries_pkce_params() {
        let cfg = ClientConfig {
            client_id: "cid".into(),
            client_secret: None,
        };
        let url = build_auth_url(&cfg, "http://127.0.0.1:9999", &["scope.a"], "chal", "st8");
        assert!(url.contains("client_id=cid"));
        assert!(url.contains("code_challenge=chal"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A9999"));
        assert!(url.contains("state=st8"));
    }

    #[test]
    fn random_verifier_meets_rfc7636_length() {
        let v = random_verifier();
        assert!((43..=128).contains(&v.len()), "len={}", v.len());
        assert!(v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')));
    }
}
