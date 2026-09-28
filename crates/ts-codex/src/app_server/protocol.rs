//! Wire framing for the Codex `app-server` newline-delimited JSON protocol.
//!
//! There is no `"jsonrpc"` field. Requests are `{"id","method","params"}`,
//! notifications are `{"method","params"}` (no `id`), and responses are
//! `{"id","result"}` or `{"id","error":{"code","message"}}`.

use serde::Serialize;
use serde_json::Value;

/// An error returned by the app-server, or synthesized locally (connection loss).
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// One parsed line from the app-server's stdout.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A successful response to one of our requests.
    Result { id: i64, result: Value },
    /// An error response to one of our requests.
    Error { id: i64, error: RpcError },
    /// The server issuing its own request; we never accept these.
    ServerRequest { id: i64, method: String },
    /// A one-way notification.
    Notification { method: String, params: Value },
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ParseError {
    #[error("invalid JSON: {0}")]
    Json(String),
    #[error("message has neither `id` nor `method`")]
    Empty,
}

/// Parse one line of app-server output. Never panics on malformed input.
pub fn parse_line(line: &str) -> Result<Incoming, ParseError> {
    let value: Value = serde_json::from_str(line).map_err(|e| ParseError::Json(e.to_string()))?;
    let id = value.get("id").and_then(Value::as_i64);
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    match (id, method) {
        (Some(id), Some(method)) => Ok(Incoming::ServerRequest { id, method }),
        (Some(id), None) => Ok(parse_response(id, &value)),
        (None, Some(method)) => Ok(Incoming::Notification {
            method,
            params: value.get("params").cloned().unwrap_or(Value::Null),
        }),
        (None, None) => Err(ParseError::Empty),
    }
}

fn parse_response(id: i64, value: &Value) -> Incoming {
    match value.get("error") {
        Some(err) => {
            let code = err.get("code").and_then(Value::as_i64).unwrap_or(-32603);
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Incoming::Error {
                id,
                error: RpcError { code, message },
            }
        }
        None => Incoming::Result {
            id,
            result: value.get("result").cloned().unwrap_or(Value::Null),
        },
    }
}

pub fn encode_request<P: Serialize>(id: i64, method: &str, params: &P) -> String {
    serde_json::json!({ "id": id, "method": method, "params": params }).to_string()
}

pub fn encode_notification<P: Serialize>(method: &str, params: &P) -> String {
    serde_json::json!({ "method": method, "params": params }).to_string()
}

pub fn encode_error_response(id: i64, code: i64, message: &str) -> String {
    serde_json::json!({ "id": id, "error": { "code": code, "message": message } }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_result_response() {
        let line = r#"{"id":2,"result":{"a":1}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Incoming::Result {
                id: 2,
                result: json!({"a": 1})
            }
        );
    }

    #[test]
    fn parses_error_response() {
        let line = r#"{"id":5,"error":{"code":-32600,"message":"bad"}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Incoming::Error {
                id: 5,
                error: RpcError {
                    code: -32600,
                    message: "bad".into()
                }
            }
        );
    }

    #[test]
    fn error_response_missing_code_defaults_to_internal() {
        let line = r#"{"id":5,"error":{"message":"bad"}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Incoming::Error {
                id: 5,
                error: RpcError {
                    code: -32603,
                    message: "bad".into()
                }
            }
        );
    }

    #[test]
    fn parses_server_request() {
        let line = r#"{"id":9,"method":"weird/request","params":{}}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Incoming::ServerRequest {
                id: 9,
                method: "weird/request".into()
            }
        );
    }

    #[test]
    fn parses_notification() {
        let line = r#"{"method":"initialized","params":null}"#;
        assert_eq!(
            parse_line(line).unwrap(),
            Incoming::Notification {
                method: "initialized".into(),
                params: Value::Null
            }
        );
    }

    #[test]
    fn rejects_message_without_id_or_method() {
        assert_eq!(parse_line(r#"{"foo":1}"#), Err(ParseError::Empty));
    }

    #[test]
    fn rejects_invalid_json_without_panicking() {
        assert!(matches!(parse_line("not json"), Err(ParseError::Json(_))));
    }

    #[test]
    fn encodes_request_and_notification() {
        let req = encode_request(1, "initialize", &json!({"a": 1}));
        assert_eq!(req, r#"{"id":1,"method":"initialize","params":{"a":1}}"#);
        let note = encode_notification("initialized", &Value::Null);
        assert_eq!(note, r#"{"method":"initialized","params":null}"#);
    }

    #[test]
    fn encodes_error_response() {
        let resp = encode_error_response(9, -32601, "method not found");
        let value: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(value["id"], 9);
        assert_eq!(value["error"]["code"], -32601);
        assert_eq!(value["error"]["message"], "method not found");
    }
}
