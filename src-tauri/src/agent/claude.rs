//! Claude Code as a child process speaking NDJSON, rather than as a TUI in a
//! pty.
//!
//! Same program and same account as [`crate::pty`]'s pane — the difference is
//! entirely in what comes back. `--output-format stream-json` emits the
//! structure behind the terminal, which is what lets the reader's question be
//! answered in the reader's own typeface.
//!
//! The MCP wiring is **not** shared with the pane, and that is a deliberate
//! divergence: the pane inherits whatever the reader registered with `claude mcp
//! add`, while this module hands the session its own `--mcp-config`. See
//! [`ClaudeSession::spawn`].
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
/// The same seven the pane pre-grants, for the same reasons ([`crate::pty`] holds
/// the long argument), but they no longer do the whole job of the policy here —
/// [`super::gate`] does, and this list is the half of it Claude Code can enforce
/// on its own. Keeping the flag as well as the gate is deliberate: a tool on
/// this list never even reaches the hook, so the common case costs no process
/// spawn and no round trip to the window.
pub const GRANTS: [&str; 7] = [
    "Read",
    "mcp__dreamd__get_stack",
    "mcp__dreamd__get_open_document",
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

/// The whole argument vector for one session, without the environment.
///
/// Pure and separate from [`ClaudeSession::spawn`] for the reason
/// [`crate::mcp::register::add_args`] is: the shape of a launch is worth
/// asserting, and every earlier test here could only reach the *consts* it is
/// built from. A flag that must never appear — `--bare`, `--strict-mcp-config` —
/// is a claim about the assembled vector, not about any one const.
///
/// Every element is a whole argument. Nothing is joined, so nothing has to be
/// quoted, and the two JSON documents are values rather than syntax (tenet 3).
fn spawn_args(
    mode: PermissionMode,
    settings: Option<&str>,
    mcp_config: Option<&str>,
) -> Vec<String> {
    let mut args = vec!["-p".to_string()];
    args.extend(STREAM_ARGS.iter().map(|s| s.to_string()));
    args.extend(mode_args(mode).iter().map(|s| s.to_string()));
    args.push("--allowed-tools".to_string());
    args.extend(GRANTS.iter().map(|s| s.to_string()));
    if let Some(settings) = settings {
        args.push("--settings".to_string());
        args.push(settings.to_string());
    }
    if let Some(mcp_config) = mcp_config {
        args.push("--mcp-config".to_string());
        args.push(mcp_config.to_string());
    }
    args
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

/// Pay [`resolve`]'s login shell now, on a thread nobody is waiting on.
///
/// The question is asked once per process and the answer is free after that —
/// but the *first* caller was `agent_spawn`, so the whole of a `$SHELL -l -i`
/// startup (300ms on a quiet machine, and however long the reader's `.zshrc`
/// takes on any other) landed on the one open of the pane where it is visible.
/// Called from `.setup()`, it is long over by then, and whichever call arrives
/// first is the one that pays: `OnceLock` makes the other free either way.
///
/// This starts **no agent**. The shell exits with its answer, and the session
/// itself is still created on the pane's first open — a launch that never opens
/// the pane still spawns no `claude` and holds no process.
pub fn warm() {
    std::thread::spawn(|| {
        resolve();
    });
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
    ///
    /// `mcp_config` is the `--mcp-config` document
    /// ([`crate::mcp::register::config_json`]), and it is what makes the six
    /// pre-granted `mcp__dreamd__*` tools in [`GRANTS`] refer to anything. Both
    /// flags take an inline JSON *string* — verified against 2.1.220 — so this
    /// is one `Command::arg` carrying a serde-built document rather than a path
    /// to a file dreamd would have to write and clean up.
    ///
    /// Handing it here rather than relying on the reader's own `claude mcp add`
    /// is the point: the path inside is the one *this* build resolved, so a
    /// session cannot inherit a registration naming a binary that has moved,
    /// and a reader who never registered anything still gets a pane whose agent
    /// can see their stack.
    ///
    /// **`--strict-mcp-config` is never passed**, and a test over [`spawn_args`]
    /// pins it: with the flag, this document becomes the only MCP config the
    /// session sees, and every other server the reader has would vanish inside
    /// dreamd's pane and nowhere else.
    pub fn spawn(
        cwd: &std::path::Path,
        mode: PermissionMode,
        settings: Option<&str>,
        mcp_config: Option<&str>,
        sink: Sink,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(resolve());
        cmd.args(spawn_args(mode, settings, mcp_config));
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
    fn exactly_seven_tools_are_pre_granted_and_none_is_a_wildcard() {
        // The pane's rule, restated where the native launch can break it: an
        // eighth grant is a deliberate line, not a silent widening.
        assert_eq!(GRANTS.len(), 7);
        assert_eq!(GRANTS[0], "Read");
        for g in GRANTS {
            assert!(!g.contains('*'), "{g} is a wildcard");
            assert!(!g.contains(' '), "{g} would split into two arguments");
        }
        assert_eq!(GRANTS.iter().filter(|g| g.starts_with("mcp__")).count(), 6);
    }

    #[test]
    fn strict_mcp_config_is_never_passed() {
        // With it, dreamd's own document becomes the *only* MCP config the
        // session sees, and every other server the reader has disappears inside
        // dreamd's pane and nowhere else — a failure they would reasonably
        // blame on the other server. The whole reason dreamd may inject itself
        // at all is that the injection merges.
        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
            PermissionMode::BypassPermissions,
        ] {
            let args = spawn_args(mode, Some("{}"), Some("{}"));
            assert!(!args.iter().any(|a| a == "--strict-mcp-config"), "{args:?}");
            // The sibling claim, where a whole vector can finally carry it:
            // `--bare` skips hooks, and dreamd's permission gate is a hook.
            assert!(!args.iter().any(|a| a == "--bare"), "{args:?}");
        }
    }

    #[test]
    fn the_two_json_documents_are_values_and_not_syntax() {
        // Each crosses as one whole argument, so a document containing spaces,
        // quotes or a semicolon is a string and never a second command. This is
        // tenet 3 for the pair of `--settings` / `--mcp-config` flags.
        let cfg = crate::mcp::register::config_json("/tmp/a b;c");
        let args = spawn_args(PermissionMode::Default, Some(r#"{"hooks":{}}"#), Some(&cfg));
        let at = |flag: &str| args.iter().position(|a| a == flag).expect(flag);
        assert_eq!(args[at("--settings") + 1], r#"{"hooks":{}}"#);
        assert_eq!(args[at("--mcp-config") + 1], cfg);
        // And both are optional in the same way, which is what lets the
        // harnesses drive a session with neither.
        let bare = spawn_args(PermissionMode::Default, None, None);
        assert!(!bare
            .iter()
            .any(|a| a == "--settings" || a == "--mcp-config"));
    }

    #[test]
    fn the_grants_are_the_arguments_after_allowed_tools() {
        // `--allowed-tools` takes them as separate arguments, so a grant that
        // grew a space would silently become two — and one of the halves might
        // be a tool. `exactly_seven_tools_…` pins the consts; this pins the
        // vector they land in.
        let args = spawn_args(PermissionMode::Plan, None, None);
        let at = args.iter().position(|a| a == "--allowed-tools").unwrap();
        assert_eq!(args[at + 1..at + 1 + GRANTS.len()], GRANTS[..]);
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
