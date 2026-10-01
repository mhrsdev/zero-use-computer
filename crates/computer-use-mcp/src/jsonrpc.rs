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

/// Parse one message, answering what isn't a usable one: not JSON
/// (-32700), or not a request, notification or response object (-32600,
/// with the message's id when it has a valid one).
pub fn parse_message(text: &str) -> Result<Incoming, Box<Response>> {
    let value: Value = serde_json::from_str(text).map_err(|e| {
        Box::new(Response::err(
            Value::Null,
            PARSE_ERROR,
            format!("parse error: {e}"),
        ))
    })?;
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
    let invalid = |why: String| Box::new(Response::err(id.clone(), INVALID_REQUEST, why));
    match obj.get("method") {
        Some(Value::String(_)) => {}
        Some(_) => return Err(invalid("`method` must be a string".into())),
        None if obj.contains_key("result") || obj.contains_key("error") => {}
        None => return Err(invalid("missing `method`".into())),
    }
    serde_json::from_value(value).map_err(|e| invalid(format!("invalid request: {e}")))
}

/// The longest message line read from stdin.
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// One line of input, read without trusting its size or encoding.
#[derive(Debug, PartialEq, Eq)]
pub enum RawLine {
    Line(Vec<u8>),
    /// Longer than the cap; it was skipped up to its newline.
    TooLong,
    Eof,
}

/// Read up to and including the next `\n`, keeping at most `max` bytes: a
/// longer line is consumed and dropped, so one bad message can't exhaust
/// memory or end the session.
pub fn read_capped_line(
    reader: &mut impl std::io::BufRead,
    max: usize,
) -> std::io::Result<RawLine> {
    let mut buf = Vec::new();
    let mut too_long = false;
    let mut any = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            if !any {
                return Ok(RawLine::Eof);
            }
            break;
        }
        any = true;
        let (len, found) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        if !too_long {
            if buf.len() + len > max {
                too_long = true;
                buf = Vec::new();
            } else {
                buf.extend_from_slice(&available[..len]);
            }
        }
        reader.consume(len);
        if found {
            break;
        }
    }
    Ok(if too_long {
        RawLine::TooLong
    } else {
        RawLine::Line(buf)
    })
}

// Standard JSON-RPC error codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

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
            code(r#"{"jsonrpc":"2.0","id":{},"method":"ping"}"#),
            (INVALID_REQUEST, Value::Null)
        );
    }

    #[test]
    fn valid_messages_parse() {
        let m = parse_message(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).unwrap();
        assert_eq!(m.method.as_deref(), Some("ping"));
        let n = parse_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        assert!(n.is_notification());
        let r = parse_message(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#).unwrap();
        assert!(r.is_response());
    }

    #[test]
    fn lines_are_capped_and_bytes_kept() {
        let mut input = std::io::Cursor::new(b"abc\n0123456789\n\xff\xfe\nend".to_vec());
        let mut read = || read_capped_line(&mut input, 6).unwrap();
        assert_eq!(read(), RawLine::Line(b"abc\n".to_vec()));
        assert_eq!(read(), RawLine::TooLong);
        assert_eq!(read(), RawLine::Line(b"\xff\xfe\n".to_vec()));
        assert_eq!(read(), RawLine::Line(b"end".to_vec()));
        assert_eq!(read(), RawLine::Eof);
    }
}
