//! The pseudo-terminal behind the embedded Claude Code pane.
//!
//! One process, one pty, held by [`AppState`](../main/struct.AppState.html) as
//! `Mutex<Option<Pty>>` and created on the pane's first open — never at boot.
//! `portable-pty` is the only dependency tree this surface brought in, and
//! everything platform-specific lives inside it.
//!
//! **Signing.** dreamd ships `hardenedRuntime: true` with no entitlements file,
//! deliberately. Allocating a pty and forking a child into it needs none: the
//! hardened runtime restricts code injection — unsigned executable memory, DYLD
//! overrides, library validation, task ports — not syscalls, and dreamd is not
//! App-Sandboxed. That was measured before this module was written, against a
//! real Developer-ID-signed bundle from `packaging/build.sh`, from the `.app`
//! and again from a session with no controlling terminal (the Finder-launch
//! shape). Don't add an entitlements file for this.
//!
//! **Output is base64.** The reader thread reads *bytes* and never looks at
//! them: a fixed-size read can and will split a multi-byte character, and decoding
//! per chunk would corrupt it. Base64 makes each chunk opaque and
//! transport-safe, and the frontend hands the decoded bytes straight to
//! `Terminal.write`, which is a stateful UTF-8 decoder and reassembles the
//! character across the boundary. This is also why nothing here is a `String`.
//!
//! **The sink is a closure, not an `AppHandle`.** Same reason
//! [`crate::notify`] takes one: it keeps this module Tauri-free, so a test can
//! drive a real pty with no window and no event loop.

use crate::config::PermissionMode;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Arc;

/// What the pane's child process produced. One variant per event the frontend
/// listens for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    /// A chunk of output, base64-encoded. Never a partial character's problem:
    /// see the module docs.
    Data(String),
    /// The child is gone. Carries the exit code when there was one; a signalled
    /// child has none.
    Exit(Option<u32>),
}

/// Somewhere to send [`PtyEvent`]s. `main.rs` passes one that emits on the
/// window; tests pass one that pushes onto a `Vec`.
pub type Sink = Arc<dyn Fn(PtyEvent) + Send + Sync>;

/// A live pty and the child running in it.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

/// What the pane's agent may do without being asked, every launch.
///
/// The reader's half of the loop is already an act of consent: highlighting a
/// passage and attaching a question to it *is* the grant. Making the agent then
/// ask permission to read the stack that question is in, or to open the file it
/// was highlighted in, is a prompt for something the reader has already said —
/// and it lands in a terminal they may not be looking at, so the send appears to
/// have gone nowhere.
///
/// So exactly two things are pre-granted: dreamd's own six MCP tools, and
/// `Read`. Nothing that writes, nothing that runs a command — an edit is still a
/// question the agent has to ask, in whatever mode the reader chose. The six
/// are spelled out rather than wildcarded so that a seventh tool added to
/// `mcp::schema` is a deliberate line here rather than a silent grant.
///
/// Named `mcp__dreamd__*` because `dreamd mcp` is registered under
/// `schema::SERVER_NAME`, which is what `mcp::register::add_args` puts on the
/// `claude mcp add` line — quoted there rather than spelled out again here, so
/// this cannot drift from the command a reader is told to run. A reader who
/// registered it under another name gets the prompts back, which is the safe
/// direction to be wrong in.
///
/// A `macro_rules!` rather than a `const` only because `concat!` takes
/// literals: this expands to one at compile time, so the four commands below
/// are still four whole string literals with nothing assembled at run time.
macro_rules! grants {
    () => {
        " --allowed-tools Read \
mcp__dreamd__get_stack mcp__dreamd__get_open_document mcp__dreamd__get_highlight \
mcp__dreamd__list_highlights mcp__dreamd__resolve_highlight \
mcp__dreamd__mark_passage"
    };
}

/// The command the pane runs in Claude Code's own default permission mode.
///
/// A **fixed** string, not a template: nothing the user or a document supplies
/// is interpolated here (tenet 3). `concat!` is a compile-time paste of two
/// literals in this file, not a runtime build — there is still no `format!`, no
/// `push_str`, and no value that becomes shell text. The login shell is the
/// wrapper rather than `claude` directly because a `.app` launched from Finder
/// inherits launchd's minimal `PATH` — `/usr/bin:/bin:/usr/sbin:/sbin` — and
/// `claude` installs to `~/.local/bin`. Spawning it directly works from a
/// terminal and fails from the Dock, which is the worse of the two failures to
/// ship.
const PANE_COMMAND: &str = concat!("exec claude", grants!());

/// The other three. Four `const`s and a `match`, rather than one `const` and a
/// flag appended to it, is what keeps tenet 3 intact: `config::PermissionMode`
/// is a closed enum, so every string that can ever reach a shell is written out
/// in this file and pinned by a test.
const PANE_COMMAND_ACCEPT_EDITS: &str =
    concat!("exec claude --permission-mode acceptEdits", grants!());
const PANE_COMMAND_PLAN: &str = concat!("exec claude --permission-mode plan", grants!());
const PANE_COMMAND_BYPASS: &str =
    concat!("exec claude --permission-mode bypassPermissions", grants!());

/// Which of the four literals a mode launches.
///
/// `Default` maps to the bare command rather than to `--permission-mode
/// default`: the flag is redundant there, and the bare form is what the pane
/// ran before this key existed, so an unset config is byte-identical to the
/// behaviour it replaced.
fn pane_command(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => PANE_COMMAND,
        PermissionMode::AcceptEdits => PANE_COMMAND_ACCEPT_EDITS,
        PermissionMode::Plan => PANE_COMMAND_PLAN,
        PermissionMode::BypassPermissions => PANE_COMMAND_BYPASS,
    }
}

/// Which model the pane's chips can switch to, mid-session.
///
/// A closed enum for the same reason [`PermissionMode`] is one: the value
/// crosses into a running Claude Code as *typed text*, so every string that can
/// ever be typed is written out in this file and pinned by a test. The frontend
/// sends one of three words and nothing else can reach [`model_line`].
///
/// Aliases rather than dated model ids (`claude-opus-5`): Claude Code resolves
/// these itself and keeps resolving them across releases, so a chip does not
/// rot the week a model is renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Model {
    Opus,
    Sonnet,
    Haiku,
}

/// The line typed into a *running* Claude Code to switch model.
///
/// A slash command rather than a `--model` flag, because the flag is read at
/// launch and a restart is a new session — the conversation in the scrollback
/// is the thing the reader is mid-way through, and switching model is not worth
/// losing it (the permission-mode select next door pays that cost only because
/// its flag genuinely cannot be changed any other way).
///
/// Safe under the other reading of the far end, the one
/// [`crate::prompt::read_line`]'s header sets out: if `claude` has exited, the
/// thing receiving this is a **login shell**, and what it gets is the word
/// `/model` — an absolute path that does not exist. It fails with "not found"
/// and does nothing. There is no metacharacter in any of these three strings
/// and no value interpolated into them (tenet 3).
pub fn model_line(model: Model) -> &'static str {
    match model {
        Model::Opus => "/model opus",
        Model::Sonnet => "/model sonnet",
        Model::Haiku => "/model haiku",
    }
}

/// The user's login shell, or a sane default. `SHELL` is what every terminal
/// emulator reads; the fallback matters only for an environment that stripped
/// it. zsh is macOS's default since Catalina, but it is not guaranteed to exist
/// on Linux at all — `/bin/sh` is the only interactive shell POSIX promises, and
/// `-l -i -c` is portable across every implementation of it.
fn login_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| DEFAULT_SHELL.into())
}

#[cfg(target_os = "macos")]
const DEFAULT_SHELL: &str = "/bin/zsh";
#[cfg(not(target_os = "macos"))]
const DEFAULT_SHELL: &str = "/bin/sh";

/// Login *and* interactive, in that order, ahead of the `-c`. Both are
/// load-bearing and a test pins them; see [`Pty::spawn`] for why `-i` is not
/// optional.
const SHELL_FLAGS: [&str; 2] = ["-l", "-i"];

impl Pty {
    /// Open a pty, spawn the pane's command in it, and start pumping output at
    /// `sink`. `cwd` is the repo root, because `dreamd mcp` finds this repo's
    /// socket by walking up from the process's working directory.
    ///
    /// `mode` selects one of four compiled-in commands — it is not a value that
    /// travels into the shell. Changing it is a restart, because Claude Code
    /// reads it once at launch.
    ///
    /// The shell is **login *and* interactive**. `-l` alone reads `.zprofile`
    /// but not `.zshrc`, and `.zshrc` is where a PATH that finds `claude`
    /// usually lives — `~/.local/bin` is the official installer's target and
    /// nothing else puts it on PATH. A `.app` from Finder inherits launchd's
    /// minimal PATH, so `-l -c` reached `exec claude` with no way to resolve it
    /// and the pane died with 127. `cargo tauri dev` inherits the terminal's
    /// PATH instead, which is exactly why the release build failed and the
    /// development build never did. `-i` costs whatever the user's rc file
    /// costs, once, before the exec.
    pub fn spawn(
        rows: u16,
        cols: u16,
        cwd: &std::path::Path,
        mode: PermissionMode,
        sink: Sink,
    ) -> Result<Self, String> {
        Self::spawn_command(
            rows,
            cols,
            cwd,
            &login_shell(),
            &[SHELL_FLAGS[0], SHELL_FLAGS[1], "-c", pane_command(mode)],
            sink,
        )
    }

    /// The general form. Public to the crate's tests only in spirit — it exists
    /// so they can drive the real machinery (openpty, spawn, the reader thread,
    /// the reaper) with `/bin/sh` instead of starting an actual Claude Code
    /// session as a side effect of `cargo test`.
    pub fn spawn_command(
        rows: u16,
        cols: u16,
        cwd: &std::path::Path,
        program: &str,
        args: &[&str],
        sink: Sink,
    ) -> Result<Self, String> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("could not open a pty: {e}"))?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        // xterm.js speaks xterm-256color. Without this the child sees whatever
        // TERM the GUI process inherited, which from Finder is nothing at all,
        // and a full-screen TUI degrades to line mode.
        cmd.env("TERM", "xterm-256color");

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("could not start `{program} {}`: {e}", args.join(" ")))?;
        // The parent must not keep the slave open, or the reader below never
        // sees EOF once the child exits and the pane never learns it died.
        drop(pair.slave);

        let killer = child.clone_killer();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("could not read from the pty: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("could not write to the pty: {e}"))?;

        let out = sink.clone();
        std::thread::spawn(move || pump(reader, out));
        // A second thread purely to reap the child. `wait` blocks, and the
        // alternative — polling `try_wait` from the reader — would either spin
        // or delay the exit event behind the next read.
        std::thread::spawn(move || {
            let code = child.wait().ok().map(|s| s.exit_code());
            sink(PtyEvent::Exit(code));
        });

        Ok(Self {
            master: pair.master,
            writer,
            killer,
        })
    }

    /// Keystrokes, straight through. Bytes, not text: the frontend sends
    /// exactly what xterm.js produced, escape sequences included.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.writer
            .write_all(bytes)
            .and_then(|_| self.writer.flush())
            .map_err(|e| format!("the pane's process is not accepting input: {e}"))
    }

    /// Tell the child its window changed. `SIGWINCH` is the child's problem;
    /// this is the ioctl that causes it.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        self.master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("could not resize the pane: {e}"))
    }

    /// End the child. Idempotent enough to call on a process that has already
    /// exited — that is an `Err` from the OS, not a panic, and the caller
    /// treats it as success because the postcondition already holds.
    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }
}

impl Drop for Pty {
    /// A dropped `Pty` must not leave a `claude` behind. Closing the master
    /// alone would send the child a `SIGHUP` it is free to ignore.
    fn drop(&mut self) {
        self.kill();
    }
}

/// Read until EOF, emitting each chunk base64-encoded.
///
/// Errors end the loop rather than being reported: every way this read fails —
/// the child exiting, the master being closed by `Drop` — is already described
/// to the frontend by the `Exit` event the other thread sends.
fn pump(mut reader: Box<dyn Read + Send>, sink: Sink) {
    // 64 KiB, because each read becomes one Tauri event: a busy TUI repaint or
    // a large paste-back at 4 KiB was hundreds of serialize+emit round trips
    // the kernel had already coalesced for us. The size changes nothing about
    // correctness — a read at any size can split a multi-byte character, which
    // is the module-doc reason the wire is base64 and the decoder is stateful.
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => sink(PtyEvent::Data(b64(&buf[..n]))),
        }
    }
}

/// RFC 4648's alphabet, written once: the encoder and the decoder have to agree
/// on it, and two copies of a 64-byte literal are two chances not to.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64, padded. Twenty lines rather than a dependency: this is the
/// whole of dreamd's need for one, and a wire format the frontend reads with
/// `atob` is not worth a crate.
pub fn b64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 {
            ALPHABET[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// The inverse, for input. Keystrokes arrive base64 too — a paste is arbitrary
/// bytes, and a lone surrogate or an invalid sequence must reach the child
/// unchanged rather than being replaced with U+FFFD by a `String` round trip.
///
/// Strict on purpose: this decodes what the frontend's `btoa` produced, so
/// anything else is a bug, not input to be salvaged.
pub fn from_b64(s: &str) -> Result<Vec<u8>, String> {
    // ALPHABET inverted, built from it at compile time so the two cannot
    // disagree; 0xFF marks a byte outside it. A table lookup per byte rather
    // than a scan of the alphabet — keystrokes never noticed, a large paste
    // did. Strictness is unchanged: '=' is filtered before lookup exactly as
    // before, and any other non-alphabet byte is still an error, not salvage.
    const REV: [u8; 256] = {
        let mut t = [0xFFu8; 256];
        let mut i = 0;
        while i < 64 {
            t[ALPHABET[i] as usize] = i as u8;
            i += 1;
        }
        t
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u8;
    for ch in s.bytes().filter(|c| *c != b'=') {
        let v = REV[ch as usize];
        if v == 0xFF {
            return Err("keystrokes were not base64".into());
        }
        acc = acc << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// RFC 4648's own vectors, so the encoder is pinned to the standard
    /// alphabet and padding rather than to itself.
    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(b64(input.as_bytes()), want, "for {input:?}");
        }
    }

    /// Every byte value, so `>> 6` and the 62/63 characters (`+` and `/`) are
    /// covered — the two an ad-hoc alphabet gets wrong.
    #[test]
    fn base64_covers_the_whole_byte_range() {
        let all: Vec<u8> = (0..=255u8).collect();
        let encoded = b64(&all);
        assert_eq!(decode(&encoded), all);
        assert!(encoded.contains('+') && encoded.contains('/'));
    }

    /// The reason output is base64 at all. A read boundary that lands inside a
    /// multi-byte character must survive the trip: neither half is valid UTF-8
    /// on its own, so anything that decoded per chunk would replace them with
    /// U+FFFD and the character would be gone by the time it reached the
    /// terminal.
    #[test]
    fn a_chunk_boundary_inside_a_multibyte_character_survives_reassembly() {
        // 4 bytes in UTF-8, split 3/1 — the worst case, and the one an emoji
        // in `claude`'s output hits every session.
        let whole = "aéあ🌱".as_bytes().to_vec();
        let mut mid_character = 0;
        for split in 1..whole.len() {
            let (a, b) = whole.split_at(split);
            if std::str::from_utf8(a).is_err() || std::str::from_utf8(b).is_err() {
                mid_character += 1;
            }
            let mut round = decode(&b64(a));
            round.extend(decode(&b64(b)));
            assert_eq!(round, whole, "split at {split}");
            assert_eq!(String::from_utf8(round).unwrap(), "aéあ🌱");
        }
        // 1 + 2 + 3 + 4 bytes: of the 9 places to cut, 6 land inside a
        // character. Asserted so a future edit to the fixture cannot quietly
        // reduce this to a test that only ever cuts on boundaries.
        assert_eq!(
            mid_character, 6,
            "the fixture must actually be cut mid-character"
        );
    }

    /// Resizing a pty whose child has gone is not an error on macOS — the
    /// master fd is still open and the ioctl still succeeds. Pinned because the
    /// plan predicted the opposite, and because the pane calls `resize` from a
    /// `ResizeObserver` that fires after the process exits.
    #[test]
    fn resizing_a_pty_whose_child_exited_is_not_a_panic() {
        let (sink, log) = recorder();
        let mut pty = sh(&["-c", "exit 0"], sink);
        wait_for(&log, |ev| matches!(ev, PtyEvent::Exit(_)));
        // `kill` on an already-dead child, then a resize and a write on the
        // corpse. None may panic; whether they succeed is the OS's business,
        // and on macOS the resize does — the master fd is still open.
        pty.kill();
        let _ = pty.resize(40, 120);
        let _ = pty.write(b"ignored\n");
    }

    /// The whole pipe, end to end, against a real pty: what the child writes
    /// comes back base64-encoded, and its exit is announced exactly once.
    #[test]
    fn a_childs_output_arrives_encoded_and_its_exit_is_announced() {
        let (sink, log) = recorder();
        let _pty = sh(&["-c", "printf 'héllo'"], sink);
        wait_for(&log, |ev| matches!(ev, PtyEvent::Exit(_)));
        // The reader thread and the reaper are separate threads, so the last
        // chunk can land just after the exit event. Give it a beat.
        std::thread::sleep(Duration::from_millis(100));

        let seen = log.lock().unwrap().clone();
        let text: String = seen
            .iter()
            .filter_map(|e| match e {
                PtyEvent::Data(d) => Some(String::from_utf8(decode(d)).unwrap()),
                _ => None,
            })
            .collect();
        assert!(text.contains("héllo"), "got {text:?}");
        assert_eq!(
            seen.iter()
                .filter(|e| matches!(e, PtyEvent::Exit(_)))
                .count(),
            1,
            "exit is announced exactly once",
        );
    }

    /// Dropping the handle must not leave the child running — the pane's close
    /// button is a drop, and an orphaned `claude` holding the repo would
    /// outlive the window that started it.
    #[test]
    fn dropping_the_handle_reaps_the_child() {
        let (sink, log) = recorder();
        // Long enough that it is unambiguously still running when dropped.
        let pty = sh(&["-c", "sleep 30"], sink);
        drop(pty);
        wait_for(&log, |ev| matches!(ev, PtyEvent::Exit(_)));
    }

    /// None of the four is ever run by a test — starting a real Claude Code
    /// session as a side effect of `cargo test` is not acceptable — so this is
    /// what holds the shape of what `spawn` passes the shell.
    ///
    /// Every mode is listed, so a fifth variant added to
    /// [`PermissionMode`] fails the `match` in `pane_command` at compile time
    /// and the exhaustiveness assertion here at run time.
    #[test]
    fn every_permission_mode_yields_a_fixed_command_with_nothing_interpolated() {
        let all = [
            (PermissionMode::Default, concat!("exec claude", grants!())),
            (
                PermissionMode::AcceptEdits,
                concat!("exec claude --permission-mode acceptEdits", grants!()),
            ),
            (
                PermissionMode::Plan,
                concat!("exec claude --permission-mode plan", grants!()),
            ),
            (
                PermissionMode::BypassPermissions,
                concat!("exec claude --permission-mode bypassPermissions", grants!()),
            ),
        ];
        for (mode, want) in all {
            let got = pane_command(mode);
            assert_eq!(got, want, "for {mode:?}");
            // Tenet 3, asserted rather than asserted-about: no interpolation
            // marker, no shell metacharacter, nothing a value could have been
            // spliced through.
            assert!(
                !got.contains('{')
                    && !got.contains('$')
                    && !got.contains('`')
                    && !got.contains(';')
                    && !got.contains('&')
                    && !got.contains('|'),
                "{got:?} carries shell syntax",
            );
        }
        // Four distinct strings: a `match` arm that fell through to the wrong
        // literal would still pass the loop above only if two agreed.
        let mut seen: Vec<&str> = all.iter().map(|(m, _)| pane_command(*m)).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 4, "each mode gets its own command");
        // The default config launches accept-edits (dreamd's choice, not
        // Claude Code's), so this is the command the pane actually runs unless
        // the user says otherwise.
        assert_eq!(
            pane_command(PermissionMode::default()),
            PANE_COMMAND_ACCEPT_EDITS,
        );
        assert!(login_shell().starts_with('/'), "an absolute program path");
    }

    /// `-i` is what reads `.zshrc`, and `.zshrc` is where the PATH that finds
    /// `claude` lives. Dropping it fails only in a bundled build launched from
    /// Finder — `cargo tauri dev` inherits a working PATH from the terminal — so
    /// nothing in a development loop would catch it.
    #[test]
    fn the_pane_shell_is_login_and_interactive() {
        assert_eq!(SHELL_FLAGS, ["-l", "-i"]);
    }

    /// Every launch pre-grants the stack and the file, and nothing else.
    ///
    /// The negative half is the one worth having: a grant list that grew an
    /// `Edit`, a `Write` or a `Bash` would mean a reader who chose "ask each
    /// time" is not being asked, and the flag is quiet enough that nothing else
    /// in the tree would notice.
    #[test]
    fn every_launch_grants_the_stack_and_the_document_and_nothing_that_writes() {
        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
            PermissionMode::BypassPermissions,
        ] {
            let cmd = pane_command(mode);
            // The grant list, and nothing before it — `--permission-mode
            // acceptEdits` has "Edit" in it, so a substring search over the
            // whole command answers the wrong question. Whole words, from the
            // flag onward.
            let (_, granted) = cmd
                .split_once(" --allowed-tools ")
                .unwrap_or_else(|| panic!("{cmd:?} grants nothing"));
            let granted: Vec<&str> = granted.split_whitespace().collect();
            for tool in [
                "Read",
                "mcp__dreamd__get_stack",
                "mcp__dreamd__get_open_document",
                "mcp__dreamd__get_highlight",
                "mcp__dreamd__list_highlights",
                "mcp__dreamd__resolve_highlight",
                "mcp__dreamd__mark_passage",
            ] {
                assert!(granted.contains(&tool), "{tool} missing from {granted:?}");
            }
            // Exactly those seven. An addition is a line in `grants!` and a
            // line here, which is the point — an eighth tool must not be able
            // to arrive as a side effect of anything.
            assert_eq!(granted.len(), 7, "{granted:?}");
            // A wildcard would grant a tool nobody has reviewed the moment
            // `mcp::schema` grows one.
            assert!(!cmd.contains('*'), "{cmd:?} wildcards the grant");
        }
    }

    /// The three chips, and the same tenet-3 assertion the launch commands get:
    /// the far end of this write may be a login shell rather than a composer.
    #[test]
    fn every_model_yields_a_fixed_line_with_nothing_interpolated() {
        let all = [
            (Model::Opus, "/model opus"),
            (Model::Sonnet, "/model sonnet"),
            (Model::Haiku, "/model haiku"),
        ];
        for (model, want) in all {
            let got = model_line(model);
            assert_eq!(got, want, "for {model:?}");
            assert!(
                !got.contains('{')
                    && !got.contains('$')
                    && !got.contains('`')
                    && !got.contains(';')
                    && !got.contains('&')
                    && !got.contains('|')
                    && !got.contains('\n')
                    && !got.contains('\r'),
                "{got:?} carries shell syntax or a submit",
            );
        }
        let mut seen: Vec<&str> = all.iter().map(|(m, _)| model_line(*m)).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 3, "each model gets its own line");
    }

    /// Drive the real machinery with a shell instead of `claude`.
    fn sh(args: &[&str], sink: Sink) -> Pty {
        Pty::spawn_command(24, 80, &std::env::temp_dir(), "/bin/sh", args, sink).expect("spawn")
    }

    type Log = Arc<Mutex<Vec<PtyEvent>>>;

    fn recorder() -> (Sink, Log) {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let out = log.clone();
        (
            Arc::new(move |ev| out.lock().unwrap().push(ev)) as Sink,
            log,
        )
    }

    fn wait_for(log: &Log, pred: impl Fn(&PtyEvent) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if log.lock().unwrap().iter().any(&pred) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "timed out waiting for an event; saw {:?}",
            log.lock().unwrap()
        );
    }

    fn decode(s: &str) -> Vec<u8> {
        from_b64(s).expect("valid base64")
    }

    /// `b64` and `from_b64` are each other's inverse for every byte string, and
    /// the decoder rejects rather than guesses. The pane's input path is the
    /// decoder's only caller, so a lenient one would turn a frontend bug into
    /// silently wrong keystrokes.
    #[test]
    fn the_decoder_is_the_encoders_inverse_and_rejects_non_base64() {
        for case in [
            vec![],
            b"a".to_vec(),
            b"\x1b[A".to_vec(),
            "🌱".as_bytes().to_vec(),
            (0..=255u8).collect(),
        ] {
            assert_eq!(from_b64(&b64(&case)).unwrap(), case);
        }
        assert!(from_b64("not base64!").is_err());
        assert!(from_b64("Zm9v").is_ok());
    }
}

/// Property sweep: the wire codec over arbitrary bytes.
///
/// The RFC vectors above pin the alphabet; this pins totality — every byte
/// sequence a pty can produce round-trips, which is the whole claim the
/// base64 framing makes (a paste is arbitrary bytes, a read boundary splits
/// characters, and nothing on the wire may care).
#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn any_bytes_round_trip(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
            prop_assert_eq!(from_b64(&b64(&bytes)).unwrap(), bytes);
        }
    }
}
