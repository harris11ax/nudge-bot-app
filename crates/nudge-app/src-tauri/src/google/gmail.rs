//! Gmail REST (read-only, 10e): `users.messages.list` + `users.messages.get`
//! (`format=metadata`) → lightweight email candidates the `connectors` module
//! turns into `suggested_triggers`. Same blocking `ureq` idiom and pure-parser
//! /thin-network split as `calendar.rs`; the scope requested is
//! `gmail.readonly` only — nudge-bot never sends, modifies, or deletes mail.

use serde_json::Value;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);
const MESSAGES_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/messages";

/// One email reduced to the metadata the connector heuristic needs. The body is
/// never fetched — subject + snippet is enough to phrase a candidate trigger,
/// and staying at `format=metadata` keeps this a strict read of headers.
pub struct EmailCandidate {
    pub message_id: String,
    pub subject: String,
    pub from: String,
    pub snippet: String,
    /// `internalDate` (Gmail's receive time) in unix seconds.
    pub internal_date_unix: i64,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(TIMEOUT)
        .timeout_read(TIMEOUT)
        .build()
}

/// `GET /users/me/messages?q=<query>&maxResults=<n>` — the ids of messages
/// matching a Gmail search query (e.g. `in:inbox is:unread newer_than:7d`).
/// Returns ids only; [`get_message`] resolves each to its metadata.
pub fn list_message_ids(
    access_token: &str,
    query: &str,
    max_results: u32,
) -> Result<Vec<String>, String> {
    let resp = agent()
        .get(MESSAGES_URL)
        .set("Authorization", &format!("Bearer {access_token}"))
        .query("q", query)
        .query("maxResults", &max_results.to_string())
        .call()
        .map_err(|e| format!("messages.list request failed: {e}"))?;
    let json: Value = resp
        .into_json()
        .map_err(|e| format!("bad messages.list response: {e}"))?;
    Ok(parse_message_ids(&json))
}

/// `GET /users/me/messages/{id}?format=metadata` restricted to the Subject/
/// From/Date headers — one candidate's worth of metadata, no body.
pub fn get_message(access_token: &str, id: &str) -> Result<EmailCandidate, String> {
    let url = format!("{MESSAGES_URL}/{id}");
    let resp = agent()
        .get(&url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .query("format", "metadata")
        .query("metadataHeaders", "Subject")
        .query("metadataHeaders", "From")
        .query("metadataHeaders", "Date")
        .call()
        .map_err(|e| format!("messages.get request failed ({id}): {e}"))?;
    let json: Value = resp
        .into_json()
        .map_err(|e| format!("bad messages.get response: {e}"))?;
    parse_message(&json).ok_or_else(|| format!("messages.get: malformed response ({id})"))
}

/// List then resolve: run the query, then fetch metadata for each hit. A single
/// message that fails to fetch is skipped rather than failing the whole batch,
/// so one deleted/expired id doesn't sink the connector run.
pub fn list_candidates(
    access_token: &str,
    query: &str,
    max_results: u32,
) -> Result<Vec<EmailCandidate>, String> {
    let ids = list_message_ids(access_token, query, max_results)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Ok(c) = get_message(access_token, &id) {
            out.push(c);
        }
    }
    Ok(out)
}

// --- pure helpers (unit-tested) ---

fn parse_message_ids(v: &Value) -> Vec<String> {
    v.get("messages")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_message(v: &Value) -> Option<EmailCandidate> {
    let message_id = v.get("id")?.as_str()?.to_string();
    let snippet = v
        .get("snippet")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let internal_date_unix = v
        .get("internalDate")
        .and_then(Value::as_str)
        // Gmail returns epoch milliseconds as a string.
        .and_then(|s| s.parse::<i64>().ok())
        .map(|ms| ms / 1000)
        .unwrap_or(0);
    let headers = v.get("payload").and_then(|p| p.get("headers"));
    let subject = header(headers, "Subject").unwrap_or_else(|| "(no subject)".to_string());
    let from = header(headers, "From").unwrap_or_default();
    Some(EmailCandidate {
        message_id,
        subject,
        from,
        snippet,
        internal_date_unix,
    })
}

/// Case-insensitive lookup in Gmail's `payload.headers` array of
/// `{name, value}` objects.
fn header(headers: Option<&Value>, name: &str) -> Option<String> {
    headers?
        .as_array()?
        .iter()
        .find(|h| {
            h.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .and_then(|h| h.get("value"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_message_ids_extracts_ids() {
        let v = json!({
            "messages": [{"id": "a1", "threadId": "t1"}, {"id": "b2", "threadId": "t2"}],
            "resultSizeEstimate": 2
        });
        assert_eq!(parse_message_ids(&v), vec!["a1", "b2"]);
    }

    #[test]
    fn parse_message_ids_missing_yields_empty() {
        assert!(parse_message_ids(&json!({})).is_empty());
    }

    #[test]
    fn parse_message_extracts_headers_and_date() {
        let v = json!({
            "id": "m1",
            "snippet": "Please submit the report by Friday",
            "internalDate": "1784000000000",
            "payload": {
                "headers": [
                    {"name": "From", "value": "boss@example.com"},
                    {"name": "Subject", "value": "Report due"},
                    {"name": "Date", "value": "Mon, 13 Jul 2026 09:00:00 -0400"}
                ]
            }
        });
        let c = parse_message(&v).unwrap();
        assert_eq!(c.message_id, "m1");
        assert_eq!(c.subject, "Report due");
        assert_eq!(c.from, "boss@example.com");
        assert_eq!(c.internal_date_unix, 1_784_000_000);
        assert!(c.snippet.contains("submit"));
    }

    #[test]
    fn parse_message_defaults_missing_subject() {
        let v = json!({"id": "m2", "payload": {"headers": []}});
        assert_eq!(parse_message(&v).unwrap().subject, "(no subject)");
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let headers = json!([{"name": "subject", "value": "hi"}]);
        assert_eq!(header(Some(&headers), "Subject"), Some("hi".to_string()));
    }
}
