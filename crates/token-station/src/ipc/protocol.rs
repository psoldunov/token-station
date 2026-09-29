//! The JSON-RPC 2.0 framing described in `docs/socket-api.md`.
//!
//! Pure: every function here turns bytes into a request or a value into a line,
//! so the wire format can be asserted without a socket.

use serde::Deserialize;
use serde_json::{Value, json};
use ts_core::alerts::{Alert, AlertKind};

/// The version this daemon speaks, reported by `GetVersion`.
pub const PROTOCOL_VERSION: u32 = 1;

/// The line is not JSON. Answered with a null id; the connection stays open.
pub const PARSE_ERROR: i32 = -32700;
/// Not a JSON-RPC request, or a line over [`MAX_LINE_BYTES`].
pub const INVALID_REQUEST: i32 = -32600;
/// Unknown method.
pub const METHOD_NOT_FOUND: i32 = -32601;
/// Invalid params; everything D-Bus reports as `InvalidArgs`.
pub const INVALID_PARAMS: i32 = -32602;
/// Internal error.
pub const INTERNAL_ERROR: i32 = -32603;
/// The request was valid and the daemon could not carry it out (D-Bus `Failed`).
pub const REQUEST_FAILED: i32 = -32000;

/// Longest accepted line, not counting the terminating newline.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// One request off the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// `None` for a JSON-RPC notification, which is answered with nothing.
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

/// Why a line could not be read as a request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequestError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("invalid request: {0}")]
    Invalid(String),
}

impl RequestError {
    #[must_use]
    pub fn code(&self) -> i32 {
        match self {
            RequestError::Parse(_) => PARSE_ERROR,
            RequestError::Invalid(_) => INVALID_REQUEST,
        }
    }
}

/// Read one line as a JSON-RPC request.
///
/// # Errors
///
/// Returns [`RequestError::Parse`] when the line is not JSON at all and
/// [`RequestError::Invalid`] when it is JSON but not a request: not an object,
/// a missing or non-string `method`, a `jsonrpc` that is not `"2.0"`, or an
/// `id` that is not a string, a number or null.
pub fn parse_request(line: &str) -> Result<Request, RequestError> {
    // Two steps, because the distinction the spec draws is exactly between them:
    // a line that is not JSON is a parse error, and JSON that is not a request
    // is an invalid request.
    let value: Value =
        serde_json::from_str(line).map_err(|error| RequestError::Parse(error.to_string()))?;
    let Some(object) = value.as_object() else {
        return Err(RequestError::Invalid("not a JSON object".into()));
    };
    match object.get("jsonrpc").and_then(Value::as_str) {
        Some("2.0") => {}
        Some(other) => {
            return Err(RequestError::Invalid(format!(
                "jsonrpc must be \"2.0\", not {other:?}"
            )));
        }
        None => return Err(RequestError::Invalid("no jsonrpc member".into())),
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Err(RequestError::Invalid("no string method member".into()));
    };
    let id = object.get("id").cloned();
    if let Some(id) = &id {
        if !(id.is_string() || id.is_number() || id.is_null()) {
            return Err(RequestError::Invalid(
                "id must be a string, a number or null".into(),
            ));
        }
    }
    Ok(Request {
        id,
        method: method.to_string(),
        params: object.get("params").cloned(),
    })
}

/// A successful response line, without its newline.
#[must_use]
pub fn success(id: &Value, result: &Value) -> String {
    line(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// An error response line, without its newline.
#[must_use]
pub fn failure(id: &Value, code: i32, message: &str, data: Option<Value>) -> String {
    let error = match data {
        Some(data) => json!({"code": code, "message": message, "data": data}),
        None => json!({"code": code, "message": message}),
    };
    line(&json!({"jsonrpc": "2.0", "id": id, "error": error}))
}

/// The `id` of a line that could not be read as a request.
///
/// A client matches answers by id, so an unusable request still has to be
/// answered with the id it carried: telling it `null` leaves whatever it sent
/// unanswered for good. `Null` is the honest answer only when the line has no
/// readable id of its own, which is what the spec says for a parse error.
#[must_use]
pub fn id_of(line: &str) -> Value {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| value.get("id").cloned())
        .filter(|id| id.is_string() || id.is_number())
        .unwrap_or(Value::Null)
}

/// A notification line (a request without an id), without its newline.
#[must_use]
pub fn notification(method: &str, params: &Value) -> String {
    line(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
}

/// `SnapshotChanged`, carrying the whole new state.
#[must_use]
pub fn snapshot_notification(revision: u64, snapshot_json: &str) -> String {
    notification("SnapshotChanged", &snapshot_params(revision, snapshot_json))
}

/// The `{revision, snapshot}` body `GetSnapshot`, `Subscribe` and
/// `SnapshotChanged` all carry.
#[must_use]
pub fn snapshot_params(revision: u64, snapshot_json: &str) -> Value {
    // The snapshot is already JSON; re-parsing it is the only way to nest it as
    // an object rather than as a string, and it is the daemon's own output, so
    // a failure here is a bug rather than bad input.
    let snapshot = serde_json::from_str::<Value>(snapshot_json).unwrap_or_else(|error| {
        tracing::error!(%error, "the published snapshot is not JSON");
        Value::Null
    });
    json!({"revision": revision, "snapshot": snapshot})
}

/// `Alert`, with both the structured fields and the daemon's own English text.
#[must_use]
pub fn alert_notification(alert: &Alert, now: i64) -> String {
    notification("Alert", &alert_params(alert, now))
}

/// The `Alert` body: structured fields for a client that words alerts itself,
/// `summary` and `body` for one that does not.
#[must_use]
pub fn alert_params(alert: &Alert, now: i64) -> Value {
    let (summary, body, _) = crate::notify::alert_text(alert, now);
    json!({
        "provider": alert.provider.as_str(),
        "providerName": alert.provider.display_name(),
        "windowId": alert.window_id,
        "windowLabel": alert.label,
        "kind": kind_name(alert.kind),
        "percent": alert.percent,
        "resetsAt": alert.resets_at,
        "summary": summary,
        "body": body,
    })
}

fn kind_name(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::Warning => "warning",
        AlertKind::Critical => "critical",
        AlertKind::Reset => "reset",
    }
}

/// Serialize one message as a single line.
///
/// `serde_json` escapes every newline inside a string, so the result never
/// contains one of its own — which is what makes the framing work.
fn line(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|error| {
        tracing::error!(%error, "cannot serialize a JSON-RPC message");
        String::from(
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"internal error"}}"#,
        )
    })
}

/// `GetHistory` parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryParams {
    pub provider: String,
    #[serde(rename = "windowId")]
    pub window_id: String,
    pub since: i64,
}

/// `SetSettings` parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct SettingsParams {
    pub settings: Value,
}

/// `IngestClaudeStatusline` parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct IngestParams {
    pub json: String,
}

/// Read the params of a request, naming the method in the error.
///
/// # Errors
///
/// Returns a message fit for an `invalid params` error when `params` is missing
/// or does not have the shape `T` wants.
pub fn params_of<T: serde::de::DeserializeOwned>(
    method: &str,
    params: Option<Value>,
) -> Result<T, String> {
    let params = params.ok_or_else(|| format!("{method} needs params"))?;
    serde_json::from_value(params).map_err(|error| format!("{method}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_core::ProviderId;

    #[test]
    fn a_well_formed_request_parses() {
        let request = parse_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"GetHistory","params":{"provider":"claude","windowId":"session","since":17}}"#,
        )
        .expect("parsed");
        assert_eq!(request.method, "GetHistory");
        assert_eq!(request.id, Some(json!(1)));
        let params: HistoryParams = params_of("GetHistory", request.params).expect("params");
        assert_eq!(params.provider, "claude");
        assert_eq!(params.window_id, "session");
        assert_eq!(params.since, 17);
    }

    #[test]
    fn a_request_without_an_id_is_a_notification() {
        let request = parse_request(r#"{"jsonrpc":"2.0","method":"Refresh"}"#).expect("parsed");
        assert_eq!(request.id, None);
    }

    #[test]
    fn broken_json_and_a_broken_request_are_different_codes() {
        assert_eq!(parse_request("{not json").unwrap_err().code(), PARSE_ERROR);
        assert_eq!(parse_request("").unwrap_err().code(), PARSE_ERROR);
        for line in [
            r#"{"id":1,"method":"Refresh"}"#,
            r#"{"jsonrpc":"1.0","id":1,"method":"Refresh"}"#,
            r#"{"jsonrpc":"2.0","id":1}"#,
            r#"{"jsonrpc":"2.0","id":{"a":1},"method":"Refresh"}"#,
            "[1,2,3]",
        ] {
            assert_eq!(
                parse_request(line).unwrap_err().code(),
                INVALID_REQUEST,
                "{line}"
            );
        }
    }

    #[test]
    fn an_unusable_request_still_gives_back_its_own_id() {
        assert_eq!(
            id_of(r#"{"jsonrpc":"1.0","id":7,"method":"Refresh"}"#),
            json!(7)
        );
        assert_eq!(id_of(r#"{"id":"abc","method":"Refresh"}"#), json!("abc"));
        // Nothing usable to answer with: a parse error, a notification, or an
        // id that is not a string or a number.
        for line in [
            "{not json",
            r#"{"jsonrpc":"2.0","method":"Refresh"}"#,
            r#"{"jsonrpc":"2.0","id":{"a":1},"method":"Refresh"}"#,
            r#"{"jsonrpc":"2.0","id":null,"method":"Refresh"}"#,
            "[1,2,3]",
        ] {
            assert_eq!(id_of(line), Value::Null, "{line}");
        }
    }

    #[test]
    fn responses_are_one_line_each() {
        let ok = success(&json!(3), &json!({"version": "0.1.0", "protocol": 1}));
        assert!(!ok.contains('\n'));
        let parsed: Value = serde_json::from_str(&ok).expect("json");
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 3);
        assert_eq!(parsed["result"]["protocol"], 1);

        let bad = failure(
            &json!(4),
            INVALID_PARAMS,
            "two problems",
            Some(json!({"problems": ["one", "two"]})),
        );
        let parsed: Value = serde_json::from_str(&bad).expect("json");
        assert_eq!(parsed["error"]["code"], -32602);
        assert_eq!(parsed["error"]["data"]["problems"][1], "two");
    }

    #[test]
    fn a_snapshot_with_a_newline_in_it_still_makes_one_line() {
        let line = snapshot_notification(7, r#"{"revision":7,"note":"a\nb"}"#);
        assert!(!line.contains('\n'), "{line}");
        let parsed: Value = serde_json::from_str(&line).expect("json");
        assert_eq!(parsed["method"], "SnapshotChanged");
        assert_eq!(parsed["params"]["revision"], 7);
        assert_eq!(parsed["params"]["snapshot"]["note"], "a\nb");
    }

    #[test]
    fn an_alert_carries_the_documented_fields() {
        let now = 1_790_596_800;
        let alert = Alert {
            provider: ProviderId::Claude,
            window_id: "session".into(),
            label: "Session".into(),
            kind: AlertKind::Warning,
            percent: 82.4,
            resets_at: Some(now + 8_040),
        };
        let params = alert_params(&alert, now);
        assert_eq!(params["provider"], "claude");
        assert_eq!(params["providerName"], "Claude Code");
        assert_eq!(params["windowId"], "session");
        assert_eq!(params["windowLabel"], "Session");
        assert_eq!(params["kind"], "warning");
        assert_eq!(params["percent"], 82.4);
        assert_eq!(params["resetsAt"], now + 8_040);
        assert_eq!(params["summary"], "Claude Code: Session at 82 %");
        assert_eq!(params["body"], "Resets in 2 h 14 min");

        let reset = Alert {
            kind: AlertKind::Reset,
            resets_at: None,
            ..alert.clone()
        };
        assert_eq!(alert_params(&reset, now)["kind"], "reset");
        assert_eq!(alert_params(&reset, now)["resetsAt"], Value::Null);
        let critical = Alert {
            kind: AlertKind::Critical,
            ..alert
        };
        assert_eq!(alert_params(&critical, now)["kind"], "critical");
    }

    #[test]
    fn missing_params_are_reported_against_the_method() {
        let error = params_of::<IngestParams>("IngestClaudeStatusline", None).unwrap_err();
        assert_eq!(error, "IngestClaudeStatusline needs params");
        let error =
            params_of::<IngestParams>("IngestClaudeStatusline", Some(json!({}))).unwrap_err();
        assert!(error.starts_with("IngestClaudeStatusline: "), "{error}");
    }
}
