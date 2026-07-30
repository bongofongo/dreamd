//! Claude Code as a child process speaking NDJSON, rather than as a TUI in a
//! pty.
//!
//! Same program, same account, same MCP registration as [`crate::pty`]'s pane —
//! the difference is entirely in what comes back. `--output-format stream-json`
//! emits the structure behind the terminal, which is what lets the reader's
//! question be answered in the reader's own typeface.
//!
//! # There is no shell here, and that is the point
//!
//! The pane runs `$SHELL -l -i -c "exec claude …"` because a `.app` launched
//! from Finder inherits launchd's minimal `PATH` and cannot find `claude` at
//! all. That reasoning still holds, but the remedy does not have to: this
//! module asks the login shell **where `claude` is**, once ([`resolve`]), and
//! then spawns that absolute path with [`std::process::Command`] and one
//! `.arg()` per argument. Nothing is ever assembled into a command line.
//!
//! That matters more here than it did there. The pane's four commands are whole
//! literals with nothing interpolated (tenet 3); this launch has to carry two
//! dreamd-minted *paths* — the gate socket and dreamd's own binary — inside a
//! `--settings` JSON document. Through a shell that would be a quoting problem
//! with a security answer. Through `Command::arg` it is not a problem at all,
//! because no shell ever sees it.
//!
//! # Two flags that are not optional
//!
//! - **`--verbose`** — without it the CLI will not stream `stream-json` under
//!   `-p`.
//! - **not `--bare`** — it is otherwise attractive (it skips hooks, LSP, plugin
//!   sync, CLAUDE.md discovery) and it skipping *hooks* is exactly why it can
//!   never be used: dreamd's permission gate **is** a `PreToolUse` hook.
//!
//! # What the reader keeps
//!
//! `session_id` is captured off the first `system/init` line so a future
//! `--resume` has something to name. Nothing uses it yet; it costs one field
//! and its absence would be discovered at the worst moment.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use super::{wire, AgentEvent, Sink};
use crate::config::PermissionMode;

/// The tools that never raise a card.
///
/// The same six the pane pre-grants, for the same reasons ([`crate::pty`] holds
/// the long argument), but they no longer do the whole job of the policy here —
/// [`super::gate`] does, and this list is the half of it Claude Code can enforce
/// on its own. Keeping the flag as well as the gate is deliberate: a tool on
/// this list never even reaches the hook, so the common case costs no process
/// spawn and no round trip to the window.
pub const GRANTS: [&str; 6] = [
    "Read",
    "mcp__dreamd__get_stack",
    "mcp__dreamd__get_highlight",
    "mcp__dreamd__list_highlights",
    "mcp__dreamd__resolve_highlight",
    "mcp__dreamd__mark_passage",
];

/// The flags that make the CLI speak structure. Fixed, and pinned by a test.
const STREAM_ARGS: [&str; 6] = [
    "--output-format",
    "stream-json",
    "--input-format",
    "stream-json",
    "--include-partial-messages",
    // Not decoration: `-p` will not stream without it.
    "--verbose",
];

/// The CLI's spelling of each permission mode.
///
/// `Default` contributes no flag at all rather than `--permission-mode default`,
/// matching [`crate::pty::pane_command`]: the flag is redundant and the bare
/// form is the one with the longest history behind it.
fn mode_args(mode: PermissionMode) -> &'static [&'static str] {
    match mode {
        PermissionMode::Default => &[],
        PermissionMode::AcceptEdits => &["--permission-mode", "acceptEdits"],
        PermissionMode::Plan => &["--permission-mode", "plan"],
        PermissionMode::BypassPermissions => &["--permission-mode", "bypassPermissions"],
    }
}

/// Where `claude` actually is, asked once of a login shell.
///
/// Cached because the answer cannot change within a process and the question
/// costs a whole shell startup — `.zshrc` included, which is the point: that is
/// where the `PATH` carrying `~/.local/bin` lives, and a `.app` from Finder has
/// no other way to learn it.
///
/// Falls back to the bare name, which is right in the case that matters for it:
/// `cargo tauri dev` inherits the terminal's `PATH`, so a shell that failed to
/// answer has probably not broken anything.
pub fn resolve() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| DEFAULT_SHELL.into());
        Command::new(shell)
            .args(["-l", "-i", "-c", "command -v claude"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "claude".into())
    })
}

#[cfg(target_os = "macos")]
const DEFAULT_SHELL: &str = "/bin/zsh";
#[cfg(not(target_os = "macos"))]
const DEFAULT_SHELL: &str = "/bin/sh";

/// One running conversation.
pub struct ClaudeSession {
    /// Behind a lock because a turn is written from the command thread and an
    /// interrupt may arrive from another. `None` once the child is gone.
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    child: Child,
    session_id: Arc<Mutex<Option<String>>>,
}

impl ClaudeSession {
    /// Start a conversation in `cwd`, delivering events to `sink`.
    ///
    /// `settings` is the `--settings` JSON document, which is where the
    /// permission hook is declared; `None` launches with no hook, which is what
    /// the harnesses want and what a build with the gate disabled would get.
    pub fn spawn(
        cwd: &std::path::Path,
        mode: PermissionMode,
        settings: Option<&str>,
        sink: Sink,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(resolve());
        cmd.arg("-p");
        cmd.args(STREAM_ARGS);
        cmd.args(mode_args(mode));
        cmd.arg("--allowed-tools");
        cmd.args(GRANTS);
        if let Some(settings) = settings {
            cmd.arg("--settings");
            cmd.arg(settings);
        }
        Self::spawn_command(cwd, cmd, sink)
    }

    /// The general form, so a test can drive the reader thread, the writer and
    /// the reaper with `/bin/sh` instead of starting a real Claude Code session
    /// as a side effect of `cargo test` — exactly the split
    /// [`crate::pty::Pty::spawn_command`] exists for.
    pub fn spawn_command(
        cwd: &std::path::Path,
        mut cmd: Command,
        sink: Sink,
    ) -> Result<Self, String> {
        cmd.current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Inherited rather than piped: the CLI's stderr is diagnostics for
            // whoever launched dreamd, and a pipe nobody drains is a child that
            // blocks once the buffer fills.
            .stderr(Stdio::inherit());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start the agent: {e}"))?;

        let stdin = child.stdin.take().ok_or("the agent has no stdin")?;
        let stdout = child.stdout.take().ok_or("the agent has no stdout")?;
        let session_id = Arc::new(Mutex::new(None));

        let seen = Arc::clone(&session_id);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                for event in wire::digest(&line) {
                    if let AgentEvent::Ready { session_id, .. } = &event {
                        *seen.lock().unwrap() = Some(session_id.clone());
                    }
                    sink(event);
                }
            }
        });

        Ok(Self {
            stdin: Arc::new(Mutex::new(Some(stdin))),
            child,
            session_id,
        })
    }

    /// Send one turn.
    pub fn send(&self, text: &str) -> Result<(), String> {
        self.write(&turn_line(text))
    }

    /// Stop the current turn without ending the conversation.
    ///
    /// The child stays up and the session keeps its id, which is the whole
    /// reason this exists rather than a kill: the reader who interrupts wants
    /// the turn back, not the transcript gone.
    pub fn interrupt(&self) -> Result<(), String> {
        self.write(&interrupt_line())
    }

    /// The id of the conversation, once the first `system/init` has landed.
    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().unwrap().clone()
    }

    fn write(&self, line: &str) -> Result<(), String> {
        let mut slot = self.stdin.lock().unwrap();
        let stdin = slot.as_mut().ok_or("the agent is not running")?;
        writeln!(stdin, "{line}").map_err(|e| format!("could not reach the agent: {e}"))?;
        stdin
            .flush()
            .map_err(|e| format!("could not reach the agent: {e}"))
    }

    /// End the conversation. Idempotent.
    pub fn kill(&mut self) {
        // Closing stdin first gives the child the chance to exit on EOF, which
        // is how it gets to flush a final `result` line.
        *self.stdin.lock().unwrap() = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The envelope one turn travels in.
///
/// Built with `serde_json` rather than `format!`, so the reader's question —
/// and the passages quoted into it, which are document content carrying
/// whatever the document carries — are escaped by the same library that will
/// parse them at the far end. A prompt containing a quote mark, a newline or a
/// lone backslash is not a thing that can go wrong here.
///
/// Pure, and separate from [`ClaudeSession::send`], so the escaping can be
/// asserted without a child process in the way.
pub fn turn_line(text: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": [{ "type": "text", "text": text }] }
    })
    .to_string()
}

/// The control message that ends a turn early.
///
/// `request_id` is fixed because dreamd never has two interrupts in flight —
/// there is one pane, one turn and one Escape key — and a counter would be a
/// piece of state whose only purpose is to look like it was needed.
pub fn interrupt_line() -> String {
    serde_json::json!({
        "type": "control_request",
        "request_id": "dreamd-interrupt",
        "request": { "subtype": "interrupt" }
    })
    .to_string()
}

impl super::Session for ClaudeSession {
    fn send(&self, text: &str) -> Result<(), String> {
        ClaudeSession::send(self, text)
    }
    fn interrupt(&self) -> Result<(), String> {
        ClaudeSession::interrupt(self)
    }
    fn kill(&mut self) {
        ClaudeSession::kill(self)
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Drive the real machinery — spawn, reader thread, writer, reaper — with a
    /// shell that emits two known lines. Never with `claude`, so `cargo test`
    /// cannot start a session or spend anything.
    fn echoing(script: &str) -> (ClaudeSession, mpsc::Receiver<AgentEvent>) {
        let (tx, rx) = mpsc::channel();
        let sink: Sink = Arc::new(move |e| {
            let _ = tx.send(e);
        });
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        let session = ClaudeSession::spawn_command(std::path::Path::new("."), cmd, sink)
            .expect("spawn /bin/sh");
        (session, rx)
    }

    #[test]
    fn lines_from_the_child_become_events() {
        let (_session, rx) = echoing(
            r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"s9","model":"m"}'; \
               printf '%s\n' '{"type":"system","subtype":"status","status":"requesting"}'"#,
        );
        let first = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(
            first,
            AgentEvent::Ready {
                session_id: "s9".into(),
                model: "m".into()
            }
        );
        let second = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(
            second,
            AgentEvent::Status {
                status: "requesting".into()
            }
        );
    }

    #[test]
    fn a_garbage_line_does_not_stop_the_stream() {
        // The leniency contract, end to end rather than in `wire` alone: the
        // reader thread must survive a line it cannot use.
        let (_session, rx) = echoing(
            r#"printf '%s\n' 'not json'; \
               printf '%s\n' '{"type":"system","subtype":"status","status":"requesting"}'"#,
        );
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            AgentEvent::Status {
                status: "requesting".into()
            }
        );
    }

    #[test]
    fn the_session_id_is_captured_for_a_future_resume() {
        let (session, rx) = echoing(
            r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"s9","model":"m"}'"#,
        );
        let _ = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(session.session_id().as_deref(), Some("s9"));
    }

    #[test]
    fn a_turn_survives_everything_a_document_can_contain() {
        // The prompt carries highlighted passages verbatim (tenet 6 labels
        // them, it does not filter them), so this envelope has to hold whatever
        // the markdown held. One line, valid JSON, text intact.
        let nasty = "a \"quoted\" ${passage}\nwith \\ backslashes\tand a \u{0} nul, ⟨unicode⟩, \
                     and </untrusted-quote> for luck";
        let line = turn_line(nasty);
        assert!(!line.contains('\n'), "an envelope must be one NDJSON line");

        let round: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(round["type"], "user");
        assert_eq!(round["message"]["role"], "user");
        assert_eq!(round["message"]["content"][0]["type"], "text");
        assert_eq!(round["message"]["content"][0]["text"], nasty);
    }

    #[test]
    fn a_slash_command_is_just_text_which_is_why_the_chips_need_no_restart() {
        // Verified against claude 2.1.220: `/model haiku` sent as an ordinary
        // user message is honoured, and the next turn's `system/init` reports
        // the new model. So the pane's three chips cost nothing here — no
        // relaunch, no `--resume`, no lost transcript.
        let round: serde_json::Value =
            serde_json::from_str(&turn_line("/model haiku")).expect("valid JSON");
        assert_eq!(round["message"]["content"][0]["text"], "/model haiku");
    }

    #[test]
    fn the_interrupt_is_the_shape_the_cli_acknowledged() {
        // Pinned against a real `control_response` with
        // `{"subtype":"success"}`; the CLI advertises `interrupt_receipt_v1` in
        // its init capabilities.
        let round: serde_json::Value = serde_json::from_str(&interrupt_line()).expect("valid JSON");
        assert_eq!(round["type"], "control_request");
        assert_eq!(round["request"]["subtype"], "interrupt");
        assert!(round["request_id"].is_string());
    }

    #[test]
    fn writing_to_a_killed_session_is_an_error_not_a_panic() {
        let (mut session, _rx) = echoing("true");
        session.kill();
        assert!(session.send("anything").is_err());
        assert!(session.interrupt().is_err());
        // And killing twice is fine.
        session.kill();
    }

    #[test]
    fn the_stream_flags_are_what_the_cli_needs() {
        // `--verbose` is not decoration: `-p` refuses to stream without it.
        assert!(STREAM_ARGS.contains(&"--verbose"));
        assert!(STREAM_ARGS.contains(&"--include-partial-messages"));
        let joined = STREAM_ARGS.join(" ");
        assert!(joined.contains("--output-format stream-json"));
        assert!(joined.contains("--input-format stream-json"));
    }

    #[test]
    fn bare_is_never_passed_because_it_would_skip_the_gate() {
        // `--bare` skips hooks, and dreamd's permission gate is a hook. This is
        // the assertion that a future flag-tidying commit has to argue with.
        assert!(!STREAM_ARGS.contains(&"--bare"));
        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
            PermissionMode::BypassPermissions,
        ] {
            assert!(!mode_args(mode).contains(&"--bare"));
        }
    }

    #[test]
    fn exactly_six_tools_are_pre_granted_and_none_is_a_wildcard() {
        // The pane's rule, restated where the native launch can break it: a
        // seventh grant is a deliberate line, not a silent widening.
        assert_eq!(GRANTS.len(), 6);
        assert_eq!(GRANTS[0], "Read");
        for g in GRANTS {
            assert!(!g.contains('*'), "{g} is a wildcard");
            assert!(!g.contains(' '), "{g} would split into two arguments");
        }
        assert_eq!(GRANTS.iter().filter(|g| g.starts_with("mcp__")).count(), 5);
    }

    #[test]
    fn no_argument_can_reach_a_shell() {
        // Tenet 3, stated as a property of this module: every argument is
        // passed through `Command::arg`, so metacharacters are inert. The test
        // is that `default` contributes no flag and the other three are two
        // plain words each — nothing assembled, nothing quoted.
        assert!(mode_args(PermissionMode::Default).is_empty());
        for mode in [
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
            PermissionMode::BypassPermissions,
        ] {
            let args = mode_args(mode);
            assert_eq!(args.len(), 2);
            assert_eq!(args[0], "--permission-mode");
            assert!(!args[1].contains(|c: char| c.is_whitespace() || "\"'$`\\;|&".contains(c)));
        }
    }
}
