//! The socket the permission hook talks to, and the `--settings` document that
//! points it here.
//!
//! # Why this is not the MCP socket
//!
//! [`crate::mcp::server`] names its socket after the **repo**, and binding it is
//! how a dreamd claims that repo — a second window on the same tree runs as a
//! secondary and does not serve. That is right for the stack, which belongs to
//! the repo. It is wrong for a permission card, which belongs to the **window
//! whose pane made the call**: routed through the repo socket, the secondary
//! window's agent would raise its cards in the primary's, where a reader who
//! never asked the question would be asked to approve it.
//!
//! So this socket is keyed on the session instead, and it carries no lock
//! semantics at all: nothing about binding it claims anything, and two windows
//! on one repo get one each.
//!
//! # The one place a shell is still involved
//!
//! Claude Code runs a hook's `command` through a shell. dreamd cannot change
//! that, so [`settings_json`] single-quotes both paths it embeds — dreamd's own
//! binary and this socket — and **refuses outright** if either could break out
//! of the quoting. Both are dreamd-minted (`current_exe`, and a hash under
//! `~/.config/dreamd/run`), so the refusal is a guard against a case that should
//! not arise rather than a filter on anything a user typed. Tenet 3 with a
//! failure mode instead of an escape hatch: no launch is better than a launch
//! whose hook line means something other than it says.

use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::gate::{Gate, Verdict, HOOK_TIMEOUT_SECS};
use crate::config;

/// How often the accept loop looks at its cancel flag. Same value, same
/// reason, and same `poll(2)` wait as the MCP server's — the hook connects per
/// gated tool call, so a sleep here taxed every one of them an average ~100ms.
const ACCEPT_POLL: Duration = Duration::from_millis(200);

/// Longest hook payload we will read. A `tool_input` carrying a whole file's
/// contents is plausible; a gigabyte is not.
const MAX_LINE: u64 = 4 * 1024 * 1024;

/// How many hex digits name a session. See [`socket_path`] for why this is 12
/// rather than the 16 `marks_file::root_hash` uses.
pub const SESSION_HEX: usize = 12;

/// `~/.config/dreamd/run/g<12hex>.sock`.
///
/// Keyed on the session rather than the repo — see the module docs.
///
/// **The name is deliberately shorter than the MCP socket's.** macOS caps
/// `sun_path` at ~104 bytes *including* the directory, and
/// `mcp::server::socket_path` already spends that budget down to a bare
/// 16-hex-plus-`.sock`; measured on a stock machine, a socket under `$TMPDIR`
/// lands at 102. A `gate-` prefix on a 16-hex name cost five more bytes than
/// that and failed to bind outright. So this is one letter and twelve digits —
/// 18 characters against MCP's 21 — which makes the budget question already
/// answered: **anywhere the MCP socket binds, this one binds too**, and a test
/// pins the inequality. The `g` still distinguishes it from the bare hashes
/// sharing the directory, and 48 bits is far more than enough to keep two live
/// windows apart.
pub fn socket_path(session: &str) -> PathBuf {
    config::config_dir()
        .join("run")
        .join(format!("g{session}.sock"))
}

/// Bind the gate socket, replacing any leftover from a crashed run.
///
/// Unlike the MCP bind, an `AddrInUse` here is **always** stale: the name
/// carries a per-session value, so nothing else can legitimately be holding it.
pub fn bind(path: &Path) -> io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            std::fs::remove_file(path)?;
            UnixListener::bind(path)?
        }
        Err(e) => return Err(e),
    };
    // The filesystem is the whole authorisation model: anyone who can connect
    // here can answer a permission prompt on the reader's behalf.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Serve until cancelled, then unlink.
///
/// Never panics. A gate that fell over would fail *closed* — every hook would
/// find nothing to connect to and deny — which is the safe direction, but it
/// would also make the pane useless, so the thread is written not to.
pub fn serve(listener: UnixListener, path: &Path, gate: Arc<Gate>, cancel: Arc<AtomicBool>) {
    if listener.set_nonblocking(true).is_err() {
        eprintln!("dreamd: the permission socket cannot be polled; not serving");
        let _ = std::fs::remove_file(path);
        return;
    }
    while !cancel.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let gate = gate.clone();
                std::thread::spawn(move || {
                    let _ = stream.set_nonblocking(false);
                    serve_connection(stream, &gate);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                crate::mcp::server::wait_readable(&listener, ACCEPT_POLL)
            }
            Err(e) => {
                eprintln!("dreamd: the permission socket stopped accepting: {e}");
                break;
            }
        }
    }
    drop(listener);
    let _ = std::fs::remove_file(path);
    // Whatever was still waiting has no window to be answered in.
    gate.close();
}

/// Start the server in a background thread, the way `mcp::server::spawn` does.
pub fn spawn(session: &str, gate: Arc<Gate>, cancel: Arc<AtomicBool>) -> io::Result<PathBuf> {
    let path = socket_path(session);
    let listener = bind(&path)?;
    let served = path.clone();
    std::thread::spawn(move || serve(listener, &served, gate, cancel));
    Ok(path)
}

/// One hook process: read its payload, rule on it, write the verdict back.
fn serve_connection(stream: UnixStream, gate: &Gate) {
    // Read to EOF rather than to a newline — the hook half-closes when its
    // payload is complete, which is the framing rule — so there is nothing here
    // for a `BufReader` to buffer.
    let source = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut line = String::new();
    if source.take(MAX_LINE).read_to_string(&mut line).is_err() {
        return;
    }

    let verdict = rule(&line, gate);
    let mut out = stream;
    let _ = writeln!(out, "{}", hook_response(verdict));
    let _ = out.flush();
}

/// Read a hook payload and rule on it.
///
/// Split out and pure-ish so the harness can drive the decision without a
/// socket. **An unparseable payload denies**: it means dreamd cannot tell what
/// it would be approving.
pub fn rule(payload: &str, gate: &Gate) -> Verdict {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) else {
        return Verdict::Deny;
    };
    let (Some(id), Some(tool)) = (
        v.get("tool_use_id").and_then(|x| x.as_str()),
        v.get("tool_name").and_then(|x| x.as_str()),
    ) else {
        return Verdict::Deny;
    };
    let input = v
        .get("tool_input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    gate.request(id, tool, input)
}

/// The JSON a `PreToolUse` hook prints to say yes or no.
///
/// The shape is Claude Code's, verified against 2.1.220: a `deny` here produced
/// a `tool_result` with `is_error: true` and an entry in the final result's
/// `permission_denials`, *while the session was running under
/// `--permission-mode bypassPermissions`*. The hook outranks the mode, which is
/// what makes this a gate rather than a suggestion.
pub fn hook_response(verdict: Verdict) -> String {
    let reason = match verdict {
        Verdict::Allow => "dreamd: the reader allowed this",
        Verdict::Deny => "dreamd: the reader did not allow this",
    };
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": verdict,
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

/// The `--settings` document that installs the hook.
///
/// `matcher: "*"` because [`super::gate::decide`] is the policy and it lives in
/// Rust — narrowing here would put half the rule in a JSON string where no test
/// can reach it. The pre-granted tools never arrive anyway: `--allowed-tools`
/// settles them before the hook is consulted.
///
/// Returns `None` if either path could escape its single quotes. See the module
/// docs: both are dreamd's own, so this is a guard rather than a filter, and a
/// refusal to launch is the correct response to it firing.
pub fn settings_json(exe: &Path, socket: &Path) -> Option<String> {
    let exe = shell_single_quoted(exe)?;
    let socket = shell_single_quoted(socket)?;
    Some(
        serde_json::json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "*",
                    "hooks": [{
                        "type": "command",
                        "command": format!("{exe} approve --socket {socket}"),
                        "timeout": HOOK_TIMEOUT_SECS,
                    }]
                }]
            }
        })
        .to_string(),
    )
}

/// Wrap a path in single quotes, or refuse.
///
/// Inside single quotes every shell metacharacter is inert and the only
/// character that matters is `'` itself. Rather than the usual `'\''` splice,
/// this refuses: dreamd mints both paths it quotes, neither can contain a
/// quote, and a refusal that never fires is easier to be sure of than an
/// escaping routine that never runs.
fn shell_single_quoted(path: &Path) -> Option<String> {
    let s = path.to_str()?;
    if s.contains('\'') || s.contains('\n') || s.contains('\0') {
        return None;
    }
    Some(format!("'{s}'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::gate::Ask;
    use std::io::BufReader;
    use std::sync::Mutex;

    fn silent() -> Arc<Gate> {
        Arc::new(Gate::new(Arc::new(|_: Ask| {})))
    }

    #[test]
    fn a_pre_granted_tool_is_allowed_from_a_real_hook_payload() {
        // The exact payload shape captured from claude 2.1.220.
        let payload = r#"{"session_id":"s","transcript_path":"/t.jsonl","cwd":"/w",
            "permission_mode":"bypassPermissions","hook_event_name":"PreToolUse",
            "tool_name":"Read","tool_input":{"file_path":"/w/a.md"},
            "tool_use_id":"toolu_01"}"#;
        assert_eq!(rule(payload, &silent()), Verdict::Allow);
    }

    #[test]
    fn a_payload_dreamd_cannot_read_denies() {
        // Every one of these means "dreamd does not know what it would be
        // approving", and that is a no.
        for payload in [
            "",
            "not json",
            "{}",
            r#"{"tool_name":"Bash"}"#,
            r#"{"tool_use_id":"t1"}"#,
            r#"{"tool_use_id":"t1","tool_name":null}"#,
            "null",
            "[]",
        ] {
            assert_eq!(
                rule(payload, &silent()),
                Verdict::Deny,
                "should deny {payload:?}"
            );
        }
    }

    #[test]
    fn the_response_is_the_shape_the_cli_acted_on() {
        let allow: serde_json::Value =
            serde_json::from_str(&hook_response(Verdict::Allow)).unwrap();
        let out = &allow["hookSpecificOutput"];
        assert_eq!(out["hookEventName"], "PreToolUse");
        assert_eq!(out["permissionDecision"], "allow");

        let deny: serde_json::Value = serde_json::from_str(&hook_response(Verdict::Deny)).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(deny["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("dreamd"));
    }

    #[test]
    fn the_settings_document_installs_a_hook_over_every_tool() {
        let json = settings_json(
            Path::new("/Applications/dreamd.app/Contents/MacOS/dreamd"),
            Path::new("/home/r/.config/dreamd/run/gate-0123456789abcdef.sock"),
        )
        .expect("dreamd-minted paths quote cleanly");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entry = &v["hooks"]["PreToolUse"][0];
        assert_eq!(entry["matcher"], "*");
        let hook = &entry["hooks"][0];
        assert_eq!(hook["type"], "command");
        assert_eq!(hook["timeout"], HOOK_TIMEOUT_SECS);
        let command = hook["command"].as_str().unwrap();
        assert!(command.contains("approve --socket"), "got {command}");
        assert!(command.contains("'/Applications/dreamd.app/Contents/MacOS/dreamd'"));
        assert!(command.contains("'/home/r/.config/dreamd/run/gate-0123456789abcdef.sock'"));
    }

    #[test]
    fn a_path_with_spaces_survives_because_it_is_quoted() {
        let json = settings_json(
            Path::new("/Users/a reader/Applications/dreamd"),
            Path::new("/tmp/gate-1.sock"),
        )
        .expect("a space is not a quoting failure");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let command = v["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(command.contains("'/Users/a reader/Applications/dreamd'"));
    }

    #[test]
    fn a_path_that_could_escape_its_quotes_refuses_to_launch() {
        // Neither path can contain these — dreamd mints both — so this guard is
        // never expected to fire. It exists because the alternative to
        // refusing is an escaping routine nobody would ever see run.
        assert_eq!(
            settings_json(Path::new("/tmp/it's here/dreamd"), Path::new("/tmp/g.sock")),
            None
        );
        assert_eq!(
            settings_json(
                Path::new("/tmp/dreamd"),
                Path::new("/tmp/a'; rm -rf ~; '.sock")
            ),
            None
        );
        assert_eq!(
            settings_json(Path::new("/tmp/dreamd\nrm -rf ~"), Path::new("/tmp/g.sock")),
            None
        );
    }

    #[test]
    fn a_metacharacter_that_is_inert_inside_quotes_is_allowed_through() {
        // The guard is about breaking *out* of the quoting, not about
        // characters being scary: `$`, backtick and `;` mean nothing inside
        // single quotes, and refusing them would fail launches that are fine.
        let json = settings_json(
            Path::new("/tmp/$HOME `x`;/dreamd"),
            Path::new("/tmp/g.sock"),
        )
        .expect("inert metacharacters are not an escape");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("'/tmp/$HOME `x`;/dreamd'"));
    }

    #[test]
    fn the_socket_is_named_per_session_not_per_repo() {
        // The whole reason this is a second socket: two windows must not share
        // a card queue. See the module docs.
        let a = socket_path(&"a".repeat(SESSION_HEX));
        let b = socket_path(&"b".repeat(SESSION_HEX));
        assert_ne!(a, b);
        assert!(a.to_string_lossy().ends_with(".sock"));
    }

    #[test]
    fn the_gate_name_fits_wherever_the_mcp_name_does() {
        // macOS caps `sun_path` at ~104 bytes including the directory, and both
        // sockets live in the same one. `mcp::server::socket_path` has already
        // spent that budget to within a couple of bytes on a stock machine, so
        // the only safe claim this module can make is a *relative* one: the
        // gate name is shorter, therefore it binds anywhere the other does.
        // (A `gate-` prefix on a 16-hex name was five bytes longer and failed
        // to bind under `$TMPDIR`, which is how this rule was found.)
        let gate = socket_path(&"a".repeat(SESSION_HEX));
        let mcp = crate::mcp::server::socket_path(Path::new("/w/notes"));
        let name = |p: &Path| p.file_name().unwrap().to_string_lossy().len();
        assert!(
            name(&gate) < name(&mcp),
            "gate {} vs mcp {}",
            name(&gate),
            name(&mcp)
        );
        assert_eq!(gate.parent(), mcp.parent(), "same directory, same budget");
    }

    #[test]
    fn the_socket_is_private_and_a_stale_one_is_replaced() {
        let dir = std::env::temp_dir().join(format!("dreamd-gate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gate-test.sock");

        let first = bind(&path).expect("bind");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "anyone who can connect can answer");

        // A crashed run leaves the inode behind; the name is per-session, so
        // nothing else can legitimately hold it and rebinding is correct.
        drop(first);
        let _second = bind(&path).expect("rebind over a leftover");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hook_gets_a_verdict_over_the_real_socket() {
        // End to end through the transport, with no window: connect, write a
        // payload, read the verdict. This is what `agent_check` does at scale.
        let dir = std::env::temp_dir().join(format!("dreamd-gate-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gate-e2e.sock");
        let listener = bind(&path).expect("bind");

        let seen: Arc<Mutex<Vec<Ask>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let gate = Arc::new(Gate::new(Arc::new(move |ask| {
            sink.lock().unwrap().push(ask);
        })));
        let cancel = Arc::new(AtomicBool::new(false));

        let served = path.clone();
        let g = gate.clone();
        let c = cancel.clone();
        std::thread::spawn(move || serve(listener, &served, g, c));

        // A granted tool needs nobody.
        let verdict = ask_over_socket(
            &path,
            r#"{"tool_use_id":"t1","tool_name":"Read","tool_input":{"file_path":"/w/a.md"}}"#,
        );
        assert_eq!(verdict["hookSpecificOutput"]["permissionDecision"], "allow");

        // One that is not raises a card, and the answer travels back.
        let g = gate.clone();
        std::thread::spawn(move || {
            while g.outstanding() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            g.answer("t2", Verdict::Deny, false);
        });
        let verdict = ask_over_socket(
            &path,
            r#"{"tool_use_id":"t2","tool_name":"Bash","tool_input":{"command":"rm -rf /"}}"#,
        );
        assert_eq!(verdict["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(seen.lock().unwrap().len(), 1, "exactly one card was raised");

        cancel.store(true, Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ask_over_socket(path: &Path, payload: &str) -> serde_json::Value {
        use std::io::BufRead;
        // The hook writes its payload and then half-closes, which is what tells
        // the server the request is complete.
        let mut stream = UnixStream::connect(path).expect("connect");
        stream.write_all(payload.as_bytes()).expect("write");
        stream
            .shutdown(std::net::Shutdown::Write)
            .expect("shutdown");
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).expect("read");
        serde_json::from_str(&reply).expect("a JSON verdict")
    }
}
