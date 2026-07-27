//! `dreamd mcp` — the stdio half. What Claude Code actually spawns.
//!
//! It speaks newline-delimited JSON-RPC on stdin/stdout and forwards **only**
//! `tools/call` to the GUI's socket. `initialize` and `tools/list` are answered
//! here, from the same compiled-in [`schema`] const the server uses.
//!
//! That split is load-bearing rather than an optimisation. Claude Code asks for
//! the tool list once, at startup, and caches it for the session. If the shim
//! proxied that call and dreamd happened not to be open at that moment, the
//! client would cache an empty tool list and the tools would stay missing for
//! the whole session — even after the user opened dreamd. Answering locally
//! means the tools are always advertised, and only a *call* can fail.
//!
//! When dreamd is closed, a call fails the way MCP asks it to: a successful
//! result carrying `isError: true` and a sentence the model can act on. A
//! JSON-RPC error would look like a broken client; this looks like what it is.

use crate::mcp::{jsonrpc, schema};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// How long to wait on a GUI that accepted the connection but is not replying.
/// Bounded so a wedged window cannot hang the agent's session indefinitely.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// Read stdin, answer on stdout, until EOF.
///
/// Returns `Ok(())` on a clean EOF — the client closing the pipe is how this
/// process is meant to end.
pub fn run(root: &Path) -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    let mut line = String::new();

    loop {
        line.clear();
        // Same cap as the socket side, for the same reason: nothing on either
        // end of this pipe should be able to make dreamd allocate without
        // bound.
        let read = (&mut reader)
            .take(jsonrpc::MAX_LINE as u64 + 1)
            .read_line(&mut line)
            .map_err(|e| format!("cannot read stdin: {e}"))?;
        if read == 0 {
            return Ok(());
        }
        if read > jsonrpc::MAX_LINE {
            let e = jsonrpc::Error::parse_error(format!(
                "line exceeds the {}-byte cap",
                jsonrpc::MAX_LINE
            ));
            write(
                &mut writer,
                &jsonrpc::Response::error(Value::Null, e).line(),
            )?;
            continue;
        }

        if let Some(reply) = jsonrpc::handle_line(&line, |request| answer(root, request)) {
            write(&mut writer, &reply)?;
        }
    }
}

fn write<W: Write>(writer: &mut W, line: &str) -> Result<(), String> {
    writer
        .write_all(line.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|e| format!("cannot write stdout: {e}"))
}

/// Answer locally, or proxy.
fn answer(root: &Path, request: &jsonrpc::Request) -> Result<Value, jsonrpc::Error> {
    match request.method.as_str() {
        "initialize" => Ok(schema::initialize_result()),
        m if m.starts_with("notifications/") => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(schema::tools_list_result()),
        "tools/call" => Ok(forward(root, request.params())),
        other => Err(jsonrpc::Error::method_not_found(other)),
    }
}

/// Forward one `tools/call`'s params to the GUI and hand back whatever it said.
///
/// One connection per call. A UDS connect is a few microseconds and an agent
/// makes a handful of calls a minute, so the alternative — holding a socket
/// open across a session and reconnecting on every kind of failure — buys
/// nothing and adds a reconnection state machine. It also means a dreamd that
/// is closed and reopened mid-session starts working again with no retry logic
/// anywhere.
///
/// Public because this is the exact wire an agent's calls travel, and
/// `examples/mcp_check.rs` drives it against a real socket.
pub fn forward(root: &Path, params: &Value) -> Value {
    let path = super::server::socket_path(root);
    let stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(_) => return unavailable(root),
    };
    let _ = stream.set_read_timeout(Some(REPLY_TIMEOUT));
    let _ = stream.set_write_timeout(Some(REPLY_TIMEOUT));

    match exchange(stream, params) {
        Ok(value) => value,
        Err(e) => failed(&e),
    }
}

/// Write the request, read one reply line, unwrap it.
fn exchange(mut stream: UnixStream, params: &Value) -> Result<Value, String> {
    let line = json!({
        "jsonrpc": jsonrpc::VERSION,
        // The socket end replies to whatever id it is given; a fixed one keeps
        // the client's own id out of a second process, and this exchange is
        // strictly one request and one reply.
        "id": 1,
        "method": "tools/call",
        "params": params,
    });
    let mut payload = serde_json::to_string(&line).map_err(|e| e.to_string())?;
    payload.push('\n');
    stream
        .write_all(payload.as_bytes())
        .and_then(|()| stream.flush())
        .map_err(|e| format!("could not send the call: {e}"))?;

    let mut reply = String::new();
    let read = BufReader::new(stream)
        .take(jsonrpc::MAX_LINE as u64 + 1)
        .read_line(&mut reply)
        .map_err(|e| format!("could not read the reply: {e}"))?;
    if read == 0 {
        return Err("dreamd closed the connection without replying".into());
    }

    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<jsonrpc::Error>,
    }
    let parsed: Reply =
        serde_json::from_str(reply.trim()).map_err(|e| format!("unreadable reply: {e}"))?;
    match (parsed.result, parsed.error) {
        (Some(result), _) => Ok(result),
        (None, Some(e)) => Err(e.to_string()),
        (None, None) => Err("dreamd replied with neither a result nor an error".into()),
    }
}

/// The result shape a tool failure takes: a *successful* JSON-RPC result whose
/// content says what went wrong. See the module doc.
fn tool_error(text: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": true,
    })
}

fn unavailable(root: &Path) -> Value {
    tool_error(format!(
        "dreamd is not open for {}. The human's queue lives in the dreamd window, \
         so there is nothing to read or resolve until they open it — say so rather \
         than looking for the marks yourself.",
        root.display()
    ))
}

fn failed(reason: &str) -> Value {
    tool_error(format!("dreamd could not answer: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, params: Value) -> jsonrpc::Request {
        serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params
        }))
        .expect("a request")
    }

    #[test]
    fn the_tool_list_is_answered_locally_so_a_closed_dreamd_still_advertises_tools() {
        // The one that matters: this must not depend on the socket at all.
        // Claude Code caches `tools/list` for the session, so proxying it would
        // mean "dreamd was closed at 09:00" silently equals "dreamd has no
        // tools until you restart Claude Code".
        let root = Path::new("/definitely/not/a/repo/with/a/socket");
        let list = answer(root, &request("tools/list", Value::Null)).expect("answered locally");
        assert_eq!(list, schema::tools_list_result());
        let init = answer(root, &request("initialize", Value::Null)).expect("answered locally");
        assert_eq!(init["serverInfo"]["name"], "dreamd");
    }

    #[test]
    fn a_call_with_no_dreamd_running_is_a_tool_error_not_a_protocol_error() {
        let root = Path::new("/definitely/not/a/repo/with/a/socket");
        let out = answer(
            root,
            &request("tools/call", json!({"name": "get_stack", "arguments": {}})),
        )
        .expect("a result, not an error");
        assert_eq!(out["isError"], true);
        let text = out["content"][0]["text"].as_str().expect("text");
        assert!(text.contains("dreamd is not open"), "{text}");
    }

    #[test]
    fn the_unavailable_notice_does_not_send_the_agent_hunting() {
        // Copy, and load-bearing: the failure mode this whole surface guards
        // against is an agent that decides to go and find the marks itself.
        let text = unavailable(Path::new("/repo"))["content"][0]["text"]
            .as_str()
            .expect("text")
            .to_string();
        assert!(
            text.contains("rather than looking for the marks yourself"),
            "{text}"
        );
    }

    #[test]
    fn an_unknown_method_is_method_not_found() {
        let err = answer(Path::new("/repo"), &request("resources/list", Value::Null))
            .expect_err("an error");
        assert_eq!(err.code, jsonrpc::METHOD_NOT_FOUND);
    }
}
