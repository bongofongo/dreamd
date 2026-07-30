//! `dreamd approve` — the process Claude Code spawns before every tool call.
//!
//! The same shape as [`crate::mcp::shim`], and for the same reason: a tiny,
//! stateless subcommand that connects to a running dreamd over a Unix socket,
//! asks it something, and prints the answer. One connection per call, no
//! reconnection state machine, no daemon.
//!
//! The difference is which way the pressure runs. The MCP shim answers the
//! handshake locally so that a dreamd that happens to be closed cannot blank an
//! agent's tool list for a whole session — it fails *open* in the direction of
//! usefulness. This one has no local answers at all and fails **closed**: a
//! socket that cannot be reached, a dreamd that has gone, a reply that is not a
//! verdict, all print a denial. Nothing here can approve anything on its own.
//!
//! It is spawned by the shell Claude Code runs hook commands through, so its
//! `--socket` argument arrives quoted by [`super::gate_server::settings_json`].

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use super::gate::{Verdict, HOOK_TIMEOUT_SECS};
use super::gate_server::hook_response;

/// A little under the timeout Claude Code was told to allow, so that a stuck
/// window produces dreamd's own denial rather than the CLI's timeout — the
/// reader is better served by "the reader did not allow this" than by a hook
/// that vanished.
const REPLY_TIMEOUT: Duration = Duration::from_secs(HOOK_TIMEOUT_SECS - 10);

/// Read the hook payload on stdin, ask the window, print the verdict.
///
/// Always exits 0 and always prints exactly one line: a hook that exits
/// non-zero or prints nothing is a hook Claude Code has to interpret, and the
/// interpretation is not dreamd's to choose.
pub fn run(socket: &Path) -> Result<(), String> {
    let mut payload = String::new();
    if std::io::stdin().read_to_string(&mut payload).is_err() {
        return say(Verdict::Deny);
    }
    say(ask(socket, &payload))
}

/// One round trip. Every failure is a denial; see the module docs.
fn ask(socket: &Path, payload: &str) -> Verdict {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        // No window, or a socket left behind by one that died. Either way
        // there is nobody to approve anything.
        return Verdict::Deny;
    };
    let _ = stream.set_read_timeout(Some(REPLY_TIMEOUT));
    let _ = stream.set_write_timeout(Some(REPLY_TIMEOUT));

    if stream.write_all(payload.as_bytes()).is_err() || stream.flush().is_err() {
        return Verdict::Deny;
    }
    // Half-close so the server knows the payload is complete without needing a
    // length prefix or a framing rule.
    if stream.shutdown(std::net::Shutdown::Write).is_err() {
        return Verdict::Deny;
    }

    let mut reply = String::new();
    if BufReader::new(stream).read_line(&mut reply).is_err() {
        return Verdict::Deny;
    }
    verdict_of(&reply)
}

/// Read a verdict back out of the server's reply.
///
/// Pure, and strict in the safe direction: only the exact word `allow`, in the
/// exact place, is an approval.
pub fn verdict_of(reply: &str) -> Verdict {
    let allowed = serde_json::from_str::<serde_json::Value>(reply)
        .ok()
        .and_then(|v| {
            v.get("hookSpecificOutput")?
                .get("permissionDecision")?
                .as_str()
                .map(|d| d == "allow")
        })
        .unwrap_or(false);
    if allowed {
        Verdict::Allow
    } else {
        Verdict::Deny
    }
}

fn say(verdict: Verdict) -> Result<(), String> {
    let mut out = std::io::stdout();
    writeln!(out, "{}", hook_response(verdict))
        .map_err(|e| format!("cannot write a verdict: {e}"))?;
    out.flush()
        .map_err(|e| format!("cannot write a verdict: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_allow_allows() {
        assert_eq!(
            verdict_of(&hook_response(Verdict::Allow)),
            Verdict::Allow,
            "the server's own allow must round trip"
        );
    }

    #[test]
    fn every_other_reply_denies() {
        for reply in [
            "",
            "not json",
            "{}",
            "null",
            r#"{"hookSpecificOutput":{}}"#,
            r#"{"hookSpecificOutput":{"permissionDecision":"deny"}}"#,
            r#"{"hookSpecificOutput":{"permissionDecision":"ask"}}"#,
            r#"{"hookSpecificOutput":{"permissionDecision":"Allow"}}"#,
            r#"{"hookSpecificOutput":{"permissionDecision":true}}"#,
            r#"{"permissionDecision":"allow"}"#,
            &hook_response(Verdict::Deny),
        ] {
            assert_eq!(verdict_of(reply), Verdict::Deny, "should deny {reply:?}");
        }
    }

    #[test]
    fn a_socket_that_is_not_there_denies_rather_than_hanging() {
        let nowhere = std::env::temp_dir().join("dreamd-no-such-gate.sock");
        let _ = std::fs::remove_file(&nowhere);
        assert_eq!(ask(&nowhere, r#"{"tool_name":"Bash"}"#), Verdict::Deny);
    }

    #[test]
    fn the_reply_timeout_lands_inside_the_cli_s_patience() {
        assert!(REPLY_TIMEOUT < Duration::from_secs(HOOK_TIMEOUT_SECS));
    }
}
