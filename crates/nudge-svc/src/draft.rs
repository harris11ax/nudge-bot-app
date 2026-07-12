//! Draft-text bridge (session 20, step 7c-i). The forthcoming `nudge-draft`
//! crate uses an LLM to write a concrete "next step" for the active task window
//! to `%LOCALAPPDATA%\nudge-bot\draft.txt`; this module is the svc-side reader
//! that folds that line into the anchor prompt.
//!
//! File contract (what `nudge-draft` must produce):
//!   * plain UTF-8 text file at `<config>/draft.txt`;
//!   * the first non-blank line is the suggestion (trailing lines ignored — the
//!     anchor strip is single-line);
//!   * an empty/whitespace-only file means "no suggestion" (fall back to the
//!     static window text);
//!   * the file's mtime marks when the draft was written — a draft older than
//!     [`DRAFT_STALENESS_SECS`] is ignored, so a producer that stopped can't
//!     pin a stale next-step on the anchor forever.
//!
//! Everything here is best-effort and degrades to `None` ("no draft"): a missing
//! file, an unreadable file, a stale draft, or an all-blank draft all mean the
//! svc keeps the rules-supplied window text. The reader never fails the loop.

use std::path::Path;
use std::time::UNIX_EPOCH;

/// A draft older than this (by file mtime) is treated as absent. Generous enough
/// to outlive a long task window, tight enough that a dead producer's stale
/// suggestion stops overriding the static text within the day. Tunable.
pub const DRAFT_STALENESS_SECS: i64 = 4 * 3600;

/// Hard cap on the drafted line length (chars). The anchor is a single strip
/// shared with three right-aligned buttons; an over-long LLM line is truncated
/// with an ellipsis rather than shown clipped mid-word.
const MAX_LEN: usize = 120;

/// Read the current draft suggestion, if any. Returns the sanitized first line
/// when `<config>/draft.txt` exists, is fresh (mtime within
/// `max_age_secs` of `now`), and holds non-blank text; `None` otherwise.
pub fn read(path: &Path, now: i64, max_age_secs: i64) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = mtime_unix(&meta)?;
    if !is_fresh(modified, now, max_age_secs) {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    sanitize(&raw)
}

/// Unix seconds of a file's last-modified time; `None` if unavailable or before
/// the epoch (clock skew — treated as "no usable timestamp").
fn mtime_unix(meta: &std::fs::Metadata) -> Option<i64> {
    let t = meta.modified().ok()?;
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

// --- pure helpers (unit-tested) ---

/// Is a draft written at `modified` still current relative to `now`? A draft
/// from the future (negative age, clock skew) counts as fresh — better to show a
/// just-written suggestion than to reject it on a jittery clock.
fn is_fresh(modified: i64, now: i64, max_age_secs: i64) -> bool {
    now - modified <= max_age_secs
}

/// Reduce raw file contents to a single displayable line, or `None` if it holds
/// nothing but whitespace. Takes the first non-blank line, trims it, and caps
/// the length on a char boundary (appending `…` when truncated) so a runaway
/// LLM response can't blow out the anchor strip.
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
    fn takes_first_non_blank_line_trimmed() {
        assert_eq!(sanitize("  draft the intro paragraph  "), Some("draft the intro paragraph".into()));
        // Leading blank lines are skipped; trailing lines ignored (single-line strip).
        assert_eq!(sanitize("\n\n  open the PR \n more"), Some("open the PR".into()));
    }

    #[test]
    fn blank_or_empty_is_none() {
        assert_eq!(sanitize(""), None);
        assert_eq!(sanitize("   \n\t\n  "), None);
    }

    #[test]
    fn overlong_line_truncated_on_char_boundary() {
        let long = "x".repeat(200);
        let out = sanitize(&long).unwrap();
        assert_eq!(out.chars().count(), MAX_LEN);
        assert!(out.ends_with('…'));
        // Multi-byte chars aren't split: a line of 200 emoji truncates cleanly.
        let emoji = "🌱".repeat(200);
        let out = sanitize(&emoji).unwrap();
        assert_eq!(out.chars().count(), MAX_LEN);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn exactly_max_len_not_truncated() {
        let line = "y".repeat(MAX_LEN);
        assert_eq!(sanitize(&line), Some(line.clone()));
    }

    #[test]
    fn freshness_window() {
        let now = 1_000_000;
        assert!(is_fresh(now - 100, now, 3600)); // recent
        assert!(is_fresh(now, now, 3600)); // just written
        assert!(is_fresh(now + 50, now, 3600)); // future (clock skew) still counts
        assert!(!is_fresh(now - 3601, now, 3600)); // one second too old
    }
}
