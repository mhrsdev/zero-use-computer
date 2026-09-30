//! Minimal JSON-RPC 2.0 for the MCP stdio transport (newline-delimited JSON).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JSONRPC: &str = "2.0";

/// An incoming message: a request/notification (has `method`) or a response
/// to something we sent (has `result`/`error`).
#[derive(Debug, Clone, Deserialize)]
pub struct Incoming {
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<Value>,
}

impl Incoming {
    pub fn is_response(&self) -> bool {
        self.method.is_none() && (self.result.is_some() || self.error.is_some())
    }
    pub fn is_notification(&self) -> bool {
        self.method.is_some() && self.id.is_none()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC,
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC,
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A request we send to the client (e.g. elicitation/create).
#[derive(Debug, Clone, Serialize)]
pub struct OutgoingRequest {
    pub jsonrpc: &'static str,
    pub id: Value,
    pub method: String,
    pub params: Value,
}

/// Parse one JSON-RPC message and check its envelope. On failure, returns the
/// error response to send back: `-32700` (id null) for invalid JSON, `-32600`
/// for a batch array, a missing/non-string `method`, a bad `id` or a
/// `jsonrpc` other than `"2.0"` (echoing the id when it is a valid one).
/// Responses from the client (`result`/`error`, no `method`) are accepted
/// leniently, since nothing can be answered to them.
pub fn parse_message(text: &str) -> Result<Incoming, Box<Response>> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| Response::err(Value::Null, PARSE_ERROR, format!("parse error: {e}")))
        .map_err(Box::new)?;
    let Value::Object(obj) = &value else {
        let why = if value.is_array() {
            "batch requests are not supported"
        } else {
            "a JSON-RPC message must be an object"
        };
        return Err(Box::new(Response::err(Value::Null, INVALID_REQUEST, why)));
    };
    let id = match obj.get("id") {
        None | Some(Value::Null) => Value::Null,
        Some(v @ (Value::String(_) | Value::Number(_))) => v.clone(),
        Some(_) => {
            return Err(Box::new(Response::err(
                Value::Null,
                INVALID_REQUEST,
                "`id` must be a string or a number",
            )));
        }
    };
    let invalid = |why: &str| Box::new(Response::err(id.clone(), INVALID_REQUEST, why.to_string()));
    match obj.get("method") {
        Some(Value::String(_)) => {
            if obj.get("jsonrpc").and_then(Value::as_str) != Some(JSONRPC) {
                return Err(invalid("`jsonrpc` must be \"2.0\""));
            }
        }
        Some(_) => return Err(invalid("`method` must be a string")),
        None if obj.contains_key("result") || obj.contains_key("error") => {}
        None => return Err(invalid("missing `method`")),
    }
    serde_json::from_value(value).map_err(|e| invalid(&format!("invalid request: {e}")))
}

// Standard JSON-RPC error codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

#[cfg(test)]
mod tests {
    use super::*;

    fn code(text: &str) -> (i64, Value) {
        let e = parse_message(text).unwrap_err();
        (e.error.unwrap().code, e.id)
    }

    #[test]
    fn envelope_errors() {
        assert_eq!(code("{not json"), (PARSE_ERROR, Value::Null));
        assert_eq!(code("[]"), (INVALID_REQUEST, Value::Null));
        assert_eq!(code("7"), (INVALID_REQUEST, Value::Null));
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":3,"method":5}"#),
            (INVALID_REQUEST, Value::from(3))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":2}"#),
            (INVALID_REQUEST, Value::from(2))
        );
        assert_eq!(
            code(r#"{"id":"a","method":"ping"}"#),
            (INVALID_REQUEST, Value::from("a"))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":{},"method":"ping"}"#),
            (INVALID_REQUEST, Value::Null)
        );
    }

    #[test]
    fn valid_messages_parse() {
        let m = parse_message(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).unwrap();
        assert_eq!(m.method.as_deref(), Some("ping"));
        let m = parse_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        assert!(m.is_notification());
        let m = parse_message(r#"{"jsonrpc":"2.0","id":"x","result":{}}"#).unwrap();
        assert!(m.is_response());
    }
}
