//! The pseudo-terminal behind the embedded Claude Code pane.
//!
//! One process, one pty, held by [`AppState`](../main/struct.AppState.html) as
//! `Mutex<Option<Pty>>` and created on the pane's first open — never at boot.
//! `portable-pty` is the only new dependency tree in step 5, and everything
//! platform-specific lives inside it.
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
//! them: a 4 KiB read can and will split a multi-byte character, and decoding
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

/// The command the pane runs in Claude Code's own default permission mode.
///
/// A **fixed** string, not a template: nothing the user or a document supplies
/// is interpolated here (tenet 3). The login shell is the wrapper rather than
/// `claude` directly because a `.app` launched from Finder inherits launchd's
/// minimal `PATH` — `/usr/bin:/bin:/usr/sbin:/sbin` — and `claude` installs to
/// `~/.local/bin`. Spawning it directly works from a terminal and fails from
/// the Dock, which is the worse of the two failures to ship.
const PANE_COMMAND: &str = "exec claude";

/// The other three. Four `const`s and a `match`, rather than one `const` and a
/// flag appended to it, is what keeps tenet 3 intact: `config::PermissionMode`
/// is a closed enum, so every string that can ever reach a shell is written out
/// in this file and pinned by a test. There is no `format!`, no `push_str` and
/// no path by which a config value — even a valid one — becomes shell text.
const PANE_COMMAND_ACCEPT_EDITS: &str = "exec claude --permission-mode acceptEdits";
const PANE_COMMAND_PLAN: &str = "exec claude --permission-mode plan";
const PANE_COMMAND_BYPASS: &str = "exec claude --permission-mode bypassPermissions";

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

/// The user's login shell, or a sane default. `SHELL` is what every terminal
/// emulator reads; the fallback matters only for an environment that stripped
/// it. zsh is macOS's default since Catalina, but it is not guaranteed to exist
/// on Linux at all — `/bin/sh` is the only interactive shell POSIX promises, and
/// `-l -c` is portable across every implementation of it.
fn login_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| DEFAULT_SHELL.into())
}

#[cfg(target_os = "macos")]
const DEFAULT_SHELL: &str = "/bin/zsh";
#[cfg(not(target_os = "macos"))]
const DEFAULT_SHELL: &str = "/bin/sh";

impl Pty {
    /// Open a pty, spawn the pane's command in it, and start pumping output at
    /// `sink`. `cwd` is the repo root, because `dreamd mcp` finds this repo's
    /// socket by walking up from the process's working directory.
    ///
    /// `mode` selects one of four compiled-in commands — it is not a value that
    /// travels into the shell. Changing it is a restart, because Claude Code
    /// reads it once at launch.
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
            &["-l", "-c", pane_command(mode)],
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
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => sink(PtyEvent::Data(b64(&buf[..n]))),
        }
    }
}

/// Standard base64, padded. Twenty lines rather than a dependency: this is the
/// whole of dreamd's need for one, and a wire format the frontend reads with
/// `atob` is not worth a crate.
pub fn b64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        out.push(A[(n >> 18 & 63) as usize] as char);
        out.push(A[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 {
            A[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            A[(n & 63) as usize] as char
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
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u8;
    for ch in s.bytes().filter(|c| *c != b'=') {
        let v = A
            .iter()
            .position(|c| *c == ch)
            .ok_or("keystrokes were not base64")? as u32;
        acc = acc << 6 | v;
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
            (PermissionMode::Default, "exec claude"),
            (
                PermissionMode::AcceptEdits,
                "exec claude --permission-mode acceptEdits",
            ),
            (PermissionMode::Plan, "exec claude --permission-mode plan"),
            (
                PermissionMode::BypassPermissions,
                "exec claude --permission-mode bypassPermissions",
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
