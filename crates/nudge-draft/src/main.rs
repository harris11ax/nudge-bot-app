//! `nudge-draft` — the LLM producer half of the draft-text bridge (step 7c-ii).
//!
//! Invoked on a schedule or by hand (NOT inside nudge-svc, which stays
//! network-free): reads the current task window from rules.toml, asks the
//! Anthropic API for one concrete next step, and atomically writes it to
//! `%LOCALAPPDATA%\nudge-bot\draft.txt`. The svc's `draft.rs` reader then folds
//! that line into the anchor prompt.
//!
//! Contract targeted (see nudge-svc `draft.rs`): a plain UTF-8 file whose first
//! non-blank line is the suggestion, written atomically (tmp + rename) so the
//! svc never reads a half-written line, with a fresh mtime so the reader's
//! staleness check passes.
//!
//! Exit status: 0 on success OR benign no-op (outside any window); non-zero only
//! on a real failure (missing API key, unreadable/invalid rules, write error).
//! Network/model failures degrade to a logged non-zero without touching the
//! existing draft — a dead producer must not clobber a good suggestion.

mod anthropic;

use std::path::{Path, PathBuf};
use windows::Win32::System::SystemInformation::GetLocalTime;

/// Env var holding the Anthropic API key.
const KEY_VAR: &str = "ANTHROPIC_API_KEY";
/// Optional model override.
const MODEL_VAR: &str = "NUDGE_MODEL";

/// Hard cap on the drafted line (chars), matching the svc reader's `MAX_LEN` so
/// what we write is what the anchor shows without further truncation.
const MAX_LEN: usize = 120;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(Outcome::Wrote(line)) => {
            eprintln!("wrote draft: {line}");
            std::process::ExitCode::SUCCESS
        }
        Ok(Outcome::NoWindow) => {
            eprintln!("no active task window; nothing to draft");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("nudge-draft: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

enum Outcome {
    Wrote(String),
    NoWindow,
}

fn run() -> Result<Outcome, String> {
    let api_key = std::env::var(KEY_VAR)
        .map_err(|_| format!("{KEY_VAR} not set"))?;
    if api_key.trim().is_empty() {
        return Err(format!("{KEY_VAR} is empty"));
    }
    let model = std::env::var(MODEL_VAR)
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| anthropic::DEFAULT_MODEL.to_string());

    let config = config_dir();
    let rules_path = config.join("rules.toml");
    let src = std::fs::read_to_string(&rules_path)
        .map_err(|e| format!("cannot read {}: {e}", rules_path.display()))?;
    let rules = nudge_core::rules::parse(&src)
        .map_err(|e| format!("invalid {}: {e:?}", rules_path.display()))?;

    let ctx = nudge_core::schedule::context(&rules, local_now());
    if !ctx.in_window {
        return Ok(Outcome::NoWindow);
    }
    let task = ctx.window_text;

    let raw = anthropic::draft_next_step(&api_key, &model, &task)?;
    let line = sanitize(&raw).ok_or_else(|| "model returned only blank text".to_string())?;

    let draft_path = config.join("draft.txt");
    atomic_write(&draft_path, &line)
        .map_err(|e| format!("cannot write {}: {e}", draft_path.display()))?;
    Ok(Outcome::Wrote(line))
}

/// `%LOCALAPPDATA%\nudge-bot` — the same config dir the svc uses.
fn config_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA"))
        .join("nudge-bot")
}

/// Local wall-clock snapshot in the shape `schedule::context` wants. Mirrors the
/// svc's `timers::local_now` (SYSTEMTIME weekday is 0=sun; core wants 0=mon).
fn local_now() -> nudge_core::schedule::LocalNow {
    let st = unsafe { GetLocalTime() };
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before 1970")
        .as_secs() as i64;
    nudge_core::schedule::LocalNow {
        unix,
        weekday: ((st.wDayOfWeek + 6) % 7) as u8,
        minutes: st.wHour as u32 * 60 + st.wMinute as u32,
    }
}

/// Write `line` to `path` atomically: fill a sibling temp file, then rename over
/// the target so a concurrent svc read never sees a partial line. The temp path
/// is process-unique (PID-suffixed) so overlapping runs can't stomp each other.
fn atomic_write(path: &Path, line: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    // Single trailing newline: a plain one-line text file.
    std::fs::write(&tmp, format!("{line}\n"))?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp); // best-effort cleanup on failure
            Err(e)
        }
    }
}

// --- pure helper (unit-tested) ---

/// Reduce raw model output to the single displayable line the contract wants:
/// first non-blank line, trimmed, capped on a char boundary with an ellipsis.
/// `None` if the output is all whitespace. Mirrors the svc reader's `sanitize`
/// so producer and consumer agree on the shape.
fn sanitize(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    if line.chars().count() <= MAX_LEN {
        return Some(line.to_string());
    }
    let mut out: String = line.chars().take(MAX_LEN - 1).collect();
    out.push('…');
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_first_non_blank_trimmed() {
        assert_eq!(sanitize("\n\n  Open the file  \nextra"), Some("Open the file".into()));
    }

    #[test]
    fn all_blank_is_none() {
        assert_eq!(sanitize("   \n\t\n"), None);
        assert_eq!(sanitize(""), None);
    }

    #[test]
    fn overlong_truncates_on_char_boundary() {
        let long = "🌱".repeat(200);
        let out = sanitize(&long).unwrap();
        assert_eq!(out.chars().count(), MAX_LEN);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn atomic_write_replaces_contents() {
        let dir = std::env::temp_dir().join(format!("nudge-draft-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("draft.txt");
        atomic_write(&path, "first step").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first step\n");
        // Overwrite is clean (no leftover temp files in the dir).
        atomic_write(&path, "second step").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second step\n");
        let leftovers = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(leftovers, 1, "temp file not cleaned up");
        std::fs::remove_dir_all(&dir).ok();
    }
}
