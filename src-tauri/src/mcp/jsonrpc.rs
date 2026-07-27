//! JSON-RPC 2.0 over newline-delimited JSON.
//!
//! Both transports dreamd speaks — the Unix socket the GUI listens on and the
//! stdio pipe the `dreamd mcp` shim talks to Claude Code over — are streams of
//! one JSON object per line. This module owns that framing and nothing else: it
//! never touches a socket, a file or a `Store`, so the two rules that actually
//! matter (a notification gets no reply, a bad line does not kill the session)
//! are testable without either end of a connection.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: &str = "2.0";

/// The longest line the framer will accept, in bytes.
///
/// An unbounded `read_line` on a socket is a memory DoS: one peer sending
/// gigabytes without a newline grows dreamd's heap until the OS kills it. 1 MiB
/// is far beyond any legitimate `tools/call`, whose largest argument is a quote
/// from a markdown file.
pub const MAX_LINE: usize = 1024 * 1024;

// The standard JSON-RPC 2.0 error codes. Named rather than inlined because the
// shim, the server and the tests all have to agree on them.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

fn default_version() -> String {
    VERSION.to_string()
}

/// One incoming call.
///
/// `id` is `Option` because that is the entire difference between a request and
/// a notification, and JSON-RPC's rule is that a notification is answered with
/// silence — not with an empty result, and not with an error even when one
/// occurred. An explicit `"id": null` deserialises to `None` and is treated the
/// same way; the spec reserves a null id for *our* replies to lines we could
/// not read an id out of.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    #[serde(default = "default_version")]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

impl Request {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    /// The params object, or `null` when the caller sent none. Handlers that
    /// want a field should go through this rather than unwrapping `params`, so
    /// "no params at all" and "params with the field missing" fail identically.
    pub fn params(&self) -> &Value {
        self.params.as_ref().unwrap_or(&Value::Null)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Error {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Error {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(PARSE_ERROR, message)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(INVALID_REQUEST, message)
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("no such method: {method}"))
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, message)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

/// One outgoing reply. `result` and `error` are mutually exclusive, which the
/// constructors are the only way to build, so an ill-formed response cannot be
/// spelled.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}

impl Response {
    pub fn result(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: VERSION,
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, error: Error) -> Self {
        Self {
            jsonrpc: VERSION,
            id,
            result: None,
            error: Some(error),
        }
    }

    /// The response as one NDJSON line, newline included — what a writer hands
    /// straight to the stream.
    pub fn line(&self) -> String {
        // Serializing a `Response` cannot fail: every field is a plain JSON
        // value. The fallback keeps a panic out of a socket thread anyway.
        match serde_json::to_string(self) {
            Ok(s) => format!("{s}\n"),
            Err(e) => format!(
                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":{INTERNAL_ERROR},"message":"{e}"}}}}"#
            ),
        }
    }
}

/// Parse one line into a [`Request`].
///
/// The three failures are distinguished on purpose: an over-long line and a
/// syntactically broken one are `PARSE_ERROR`, while well-formed JSON that is
/// not a JSON-RPC request is `INVALID_REQUEST`. That is the difference between
/// "your bytes are corrupt" and "your bytes arrived intact and say the wrong
/// thing", and it is the only diagnostic the far end gets.
pub fn parse(line: &str) -> Result<Request, Error> {
    if line.len() > MAX_LINE {
        return Err(Error::parse_error(format!(
            "line of {} bytes exceeds the {MAX_LINE}-byte cap",
            line.len()
        )));
    }
    let value: Value =
        serde_json::from_str(line).map_err(|e| Error::parse_error(format!("invalid JSON: {e}")))?;
    let request: Request = serde_json::from_value(value)
        .map_err(|e| Error::invalid_request(format!("not a JSON-RPC request: {e}")))?;
    if request.jsonrpc != VERSION {
        return Err(Error::invalid_request(format!(
            "unsupported jsonrpc version: {}",
            request.jsonrpc
        )));
    }
    Ok(request)
}

/// Read one line, run `dispatch`, and produce the line to write back — or
/// `None` when nothing should be written.
///
/// This is the whole read side of both transports, and it is a free function
/// taking a closure precisely so the connection loop above it has no decisions
/// left to make. `None` happens for a blank line and for a notification;
/// everything else, including every kind of failure, produces a reply. **A
/// malformed line must never end the connection** — Claude Code holds one
/// process open for a whole session, and dropping it over a stray byte would
/// take the session's tools with it.
pub fn handle_line<F>(line: &str, dispatch: F) -> Option<String>
where
    F: FnOnce(&Request) -> Result<Value, Error>,
{
    if line.trim().is_empty() {
        return None;
    }
    let request = match parse(line) {
        Ok(r) => r,
        // The id is unreadable by definition here, so the spec's null it is.
        Err(e) => return Some(Response::error(Value::Null, e).line()),
    };
    // Dispatch runs before the notification check: a notification's side
    // effects still happen, it is only the reply that is suppressed.
    let outcome = dispatch(&request);
    let id = request.id.clone()?;
    Some(
        match outcome {
            Ok(result) => Response::result(id, result),
            Err(error) => Response::error(id, error),
        }
        .line(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok(_: &Request) -> Result<Value, Error> {
        Ok(json!({"fine": true}))
    }

    fn reply(line: &str) -> Value {
        let out = handle_line(line, ok).expect("a reply was expected");
        assert!(out.ends_with('\n'), "NDJSON lines are newline-terminated");
        serde_json::from_str(&out).expect("the reply is valid JSON")
    }

    #[test]
    fn a_request_with_no_id_is_a_notification_and_gets_no_reply() {
        // `notifications/initialized` is the one Claude Code sends immediately
        // after `initialize`. Answering it is a protocol violation, and a
        // client that gets an unexpected reply can wedge.
        let out = handle_line(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            ok,
        );
        assert_eq!(out, None);
        // An explicit null id is the same thing.
        let out = handle_line(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#, ok);
        assert_eq!(out, None);
    }

    #[test]
    fn a_notification_still_runs_its_handler() {
        // Silence is about the reply, not about the work.
        let mut ran = false;
        let out = handle_line(r#"{"jsonrpc":"2.0","method":"ping"}"#, |_| {
            ran = true;
            Ok(Value::Null)
        });
        assert_eq!(out, None);
        assert!(ran, "the handler must still have been called");
    }

    #[test]
    fn a_notification_that_fails_is_still_answered_with_silence() {
        let out = handle_line(r#"{"jsonrpc":"2.0","method":"boom"}"#, |_| {
            Err(Error::internal("exploded"))
        });
        assert_eq!(
            out, None,
            "an error must not turn a notification into a reply"
        );
    }

    #[test]
    fn a_malformed_line_produces_a_parse_error_not_a_disconnect() {
        // The point of the test is the `Some`: one bad byte on the wire must
        // cost a reply, not a Claude Code session.
        let v = reply("{not json at all");
        assert_eq!(v["error"]["code"], PARSE_ERROR);
        assert_eq!(v["id"], Value::Null, "an unreadable id replies with null");
        assert!(v.get("result").is_none(), "no result key beside an error");

        // Valid JSON that is not a request is a different code.
        let v = reply(r#"{"jsonrpc":"2.0","id":1}"#);
        assert_eq!(v["error"]["code"], INVALID_REQUEST, "{v}");

        // ...and so is the wrong protocol version.
        let v = reply(r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#);
        assert_eq!(v["error"]["code"], INVALID_REQUEST, "{v}");
        assert!(
            v["error"]["message"].as_str().unwrap().contains("1.0"),
            "{v}"
        );
    }

    #[test]
    fn a_line_longer_than_the_cap_is_refused() {
        // An unbounded read on a socket is a memory DoS. The refusal is a
        // reply, so the far end learns why rather than watching a hang.
        let huge = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"x":"{}"}}}}"#,
            "a".repeat(MAX_LINE)
        );
        assert!(huge.len() > MAX_LINE);
        let err = parse(&huge).expect_err("must be refused");
        assert_eq!(err.code, PARSE_ERROR);
        assert!(err.message.contains("cap"), "{err}");

        // A line just under the cap is still perfectly acceptable.
        let fine = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"x":"{}"}}}}"#,
            "a".repeat(1024)
        );
        assert!(parse(&fine).is_ok());
    }

    #[test]
    fn a_blank_line_is_ignored_rather_than_answered() {
        // Streams end with a newline; a keepalive or a trailing separator must
        // not produce a parse error for the client to log.
        assert_eq!(handle_line("", ok), None);
        assert_eq!(handle_line("   \t ", ok), None);
    }

    #[test]
    fn a_reply_carries_back_the_id_it_was_asked_with() {
        // Ids are opaque: a number, a string, anything. Rewriting one loses the
        // client's correlation.
        for id in [json!(7), json!("abc"), json!(0)] {
            let line = json!({"jsonrpc":"2.0","id":id,"method":"ping"}).to_string();
            let v = reply(&line);
            assert_eq!(v["id"], id);
            assert_eq!(v["jsonrpc"], "2.0");
            assert_eq!(v["result"]["fine"], true);
            assert!(v.get("error").is_none());
        }
    }

    #[test]
    fn absent_params_read_as_null_rather_than_panicking() {
        let r = parse(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).expect("parses");
        assert_eq!(*r.params(), Value::Null);
        assert!(!r.is_notification());
        // A missing `jsonrpc` is tolerated and defaulted; a wrong one is not.
        let r = parse(r#"{"id":1,"method":"ping"}"#).expect("parses");
        assert_eq!(r.jsonrpc, VERSION);
    }

    #[test]
    fn an_error_from_the_handler_becomes_an_error_reply() {
        let out = handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#, |r| {
            Err(Error::method_not_found(&r.method))
        })
        .expect("a reply");
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(v["error"]["message"], "no such method: nope");
        assert_eq!(v["id"], 3);
    }
}
