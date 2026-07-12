//! Minimal Anthropic Messages API client (raw HTTP — there is no official Rust
//! SDK, so per the claude-api guidance this uses the documented REST shape).
//! Blocking `ureq`, no async, matching the workspace convention.
//!
//! The task is deliberately tiny: turn a human-authored task description into a
//! single concrete next action. So thinking is left off (the model runs without
//! extended thinking on Opus 4.8 when the `thinking` field is omitted) and
//! `max_tokens` is small — we want one terse line, fast and cheap.

use serde_json::Value;
use std::time::Duration;

/// Anthropic Messages endpoint.
const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

/// Pinned API version header value (the current stable Messages API version).
const API_VERSION: &str = "2023-06-01";

/// Default model: the latest, most capable Opus. Overridable via `NUDGE_MODEL`
/// so a cheaper/faster model can be swapped in without a rebuild.
pub const DEFAULT_MODEL: &str = "claude-opus-4-8";

/// One drafting call is a single small request; a generous-but-bounded timeout
/// keeps a hung network from wedging a scheduled run indefinitely.
const TIMEOUT: Duration = Duration::from_secs(30);

/// System prompt: pins the model to a single-line, imperative next-step with no
/// preamble, so the raw first line already satisfies the anchor-strip contract.
const SYSTEM: &str = "You help someone start a task they have been putting off. \
Given the task, reply with ONE short, concrete first action they can take in \
the next two minutes to get moving. Imperative voice, under 100 characters, no \
preamble, no quotes, no trailing punctuation. Output only that single line.";

/// Draft a concrete next-step for `task`. Returns the sanitized suggestion line,
/// or an `Err(message)` describing why no draft could be produced (missing key,
/// network/parse failure, refusal, empty output). Callers treat `Err` as "leave
/// the existing draft untouched".
pub fn draft_next_step(api_key: &str, model: &str, task: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(TIMEOUT)
        .timeout_read(TIMEOUT)
        .build();
    let body = build_body(model, task);

    let resp = agent
        .post(ENDPOINT)
        .set("x-api-key", api_key)
        .set("anthropic-version", API_VERSION)
        .set("content-type", "application/json")
        .send_json(body)
        .map_err(|e| format!("request failed: {e}"))?;

    let json: Value = resp
        .into_json()
        .map_err(|e| format!("bad response body: {e}"))?;

    extract_text(&json).ok_or_else(|| "no usable text in response".to_string())
}

// --- pure helpers (unit-tested) ---

/// Build the Messages API request body for a one-line next-step.
fn build_body(model: &str, task: &str) -> Value {
    serde_json::json!({
        "model": model,
        "max_tokens": 64,
        "system": SYSTEM,
        "messages": [{
            "role": "user",
            "content": format!("Task: {task}\n\nWhat is the single next action?"),
        }],
    })
}

/// Pull the first text block out of a Messages API response, or `None` if the
/// model refused (`stop_reason == "refusal"`) or produced no text block.
fn extract_text(resp: &Value) -> Option<String> {
    if resp.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        return None;
    }
    resp.get("content")?
        .as_array()?
        .iter()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .and_then(|b| b.get("text").and_then(Value::as_str))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn body_carries_model_and_task() {
        let b = build_body("claude-opus-4-8", "write the intro");
        assert_eq!(b["model"], "claude-opus-4-8");
        assert_eq!(b["messages"][0]["role"], "user");
        assert!(b["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("write the intro"));
        // No `thinking` field — thinking stays off for this tiny task.
        assert!(b.get("thinking").is_none());
    }

    #[test]
    fn extracts_first_text_block() {
        let r = json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "Open the doc and write one sentence"}],
        });
        assert_eq!(
            extract_text(&r).as_deref(),
            Some("Open the doc and write one sentence")
        );
    }

    #[test]
    fn skips_non_text_blocks() {
        let r = json!({
            "content": [
                {"type": "thinking", "thinking": "..."},
                {"type": "text", "text": "Start here"}
            ]
        });
        assert_eq!(extract_text(&r).as_deref(), Some("Start here"));
    }

    #[test]
    fn refusal_yields_none() {
        let r = json!({
            "stop_reason": "refusal",
            "content": [{"type": "text", "text": "leaked"}],
        });
        assert_eq!(extract_text(&r), None);
    }

    #[test]
    fn missing_or_empty_content_yields_none() {
        assert_eq!(extract_text(&json!({})), None);
        assert_eq!(extract_text(&json!({"content": []})), None);
        assert_eq!(extract_text(&json!({"content": [{"type": "image"}]})), None);
    }
}
