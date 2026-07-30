//! Correctness harness for the permission gate: a real socket, the real
//! `dreamd approve` binary as the client, and no window anywhere. Exits
//! non-zero on the first failure.
//!
//! An example rather than a `#[cfg(test)]` module for the reason `CLAUDE.md`
//! gives and `mcp_check` follows: the socket lives under `config_dir()`, which
//! reads the real `~/.config/dreamd`, and `cargo test` is not allowed near it.
//! `XDG_CONFIG_HOME` is repointed process-wide before anything runs.
//!
//! What is **not** here: everything `gate.rs` and `gate_server.rs` already
//! assert without a transport — the allowlist, the always-grant, the quoting
//! guard, the response shape. This harness exists for what only a real socket
//! and a real second process can show:
//!
//! - `dreamd approve` speaks the protocol the server speaks — the two halves
//!   are compiled from the same crate but they have never been run against each
//!   other until here;
//! - a hook whose window has gone denies rather than hanging;
//! - a hook that outlives the reader's patience denies, and does so *by itself*
//!   rather than by being killed;
//! - a card raised by one hook is answered while a second is still waiting, so
//!   two outstanding cards do not interfere.
//!
//! ```sh
//! cargo run --example agent_check
//! ```

use dreamd::agent::gate::{Ask, Gate, Verdict};
use dreamd::agent::gate_server;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() {
    let root = std::env::temp_dir().join("dreamd-agent-chk");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp dir");
    // Before anything reaches `config_dir()`.
    std::env::set_var("XDG_CONFIG_HOME", root.join("xdg"));

    let mut c = Checks::default();
    let exe = dreamd_binary();
    println!("driving {}", exe.display());

    let cards: Arc<Mutex<Vec<Ask>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = cards.clone();
    let gate = Arc::new(Gate::new(Arc::new(move |ask| {
        sink.lock().unwrap().push(ask);
    })));
    let cancel = Arc::new(AtomicBool::new(false));
    let socket = gate_server::spawn(
        &"a".repeat(gate_server::SESSION_HEX),
        gate.clone(),
        cancel.clone(),
    )
    .expect("bind the gate socket");

    // -- the two halves agree on the wire ---------------------------------

    // A pre-granted tool never troubles the reader, and the answer comes back
    // through the real binary.
    let out = approve(
        &exe,
        &socket,
        payload("t1", "Read", r#"{"file_path":"/w/a.md"}"#),
    );
    c.eq(
        "a granted tool is allowed end to end",
        decision(&out),
        "allow",
    );
    c.eq("and raised no card", cards.lock().unwrap().len(), 0);

    // One that is not granted raises a card, and an allow travels back.
    answer_when_asked(&gate, "t2", Verdict::Allow, false);
    let out = approve(
        &exe,
        &socket,
        payload("t2", "Bash", r#"{"command":"cargo build"}"#),
    );
    c.eq(
        "an allowed card is allowed end to end",
        decision(&out),
        "allow",
    );
    c.eq(
        "and the card carried the call",
        card_tool(&cards, "t2"),
        Some("Bash".to_string()),
    );

    // And a deny.
    answer_when_asked(&gate, "t3", Verdict::Deny, false);
    let out = approve(
        &exe,
        &socket,
        payload("t3", "Write", r#"{"file_path":"/w/a.md"}"#),
    );
    c.eq("a denied card is denied end to end", decision(&out), "deny");

    c.ok(
        "the reason names dreamd, so the agent can report who refused",
        out.get("hookSpecificOutput")
            .and_then(|o| o.get("permissionDecisionReason"))
            .and_then(|r| r.as_str())
            .is_some_and(|r| r.contains("dreamd")),
    );

    // -- an "always" grant survives the process boundary -------------------

    answer_when_asked(&gate, "t4", Verdict::Allow, true);
    let out = approve(
        &exe,
        &socket,
        payload("t4", "Edit", r#"{"file_path":"/w/a.md"}"#),
    );
    c.eq(
        "the first Edit is allowed by the reader",
        decision(&out),
        "allow",
    );
    let before = cards.lock().unwrap().len();
    let out = approve(
        &exe,
        &socket,
        payload("t5", "Edit", r#"{"file_path":"/w/b.md"}"#),
    );
    c.eq(
        "the next Edit is allowed by the grant",
        decision(&out),
        "allow",
    );
    c.eq(
        "and raised no second card",
        cards.lock().unwrap().len(),
        before,
    );

    // -- two cards at once do not interfere --------------------------------

    let (exe2, socket2) = (exe.clone(), socket.clone());
    let first = std::thread::spawn(move || {
        approve(
            &exe2,
            &socket2,
            payload("t6", "Bash", r#"{"command":"one"}"#),
        )
    });
    let (exe3, socket3) = (exe.clone(), socket.clone());
    let second = std::thread::spawn(move || {
        approve(
            &exe3,
            &socket3,
            payload("t7", "Task", r#"{"description":"two"}"#),
        )
    });
    wait_until(|| gate.outstanding() == 2, Duration::from_secs(10));
    c.eq("two hooks wait independently", gate.outstanding(), 2);
    // Answer them out of order, which is what a reader clicking a stack of
    // cards actually does.
    gate.answer("t7", Verdict::Allow, false);
    gate.answer("t6", Verdict::Deny, false);
    c.eq(
        "the first hook got its own verdict",
        decision(&first.join().unwrap()),
        "deny",
    );
    c.eq(
        "the second hook got its own verdict",
        decision(&second.join().unwrap()),
        "allow",
    );

    // -- every kind of silence is a denial ---------------------------------

    let nowhere = root.join("no-such-gate.sock");
    let out = approve(&exe, &nowhere, payload("t8", "Bash", "{}"));
    c.eq("a socket with no window denies", decision(&out), "deny");

    let out = approve(&exe, &socket, "not json at all".into());
    c.eq(
        "a payload dreamd cannot read denies",
        decision(&out),
        "deny",
    );

    let out = approve(&exe, &socket, r#"{"tool_name":"Bash"}"#.into());
    c.eq(
        "a payload with no tool_use_id denies",
        decision(&out),
        "deny",
    );

    // -- retiring the server denies what was waiting ------------------------

    let (exe4, socket4) = (exe.clone(), socket.clone());
    let orphan = std::thread::spawn(move || {
        approve(
            &exe4,
            &socket4,
            payload("t9", "Bash", r#"{"command":"orphan"}"#),
        )
    });
    wait_until(|| gate.outstanding() == 1, Duration::from_secs(10));
    cancel.store(true, Ordering::Relaxed);
    let out = orphan.join().unwrap();
    c.eq(
        "a card whose window went away denies",
        decision(&out),
        "deny",
    );

    wait_until(|| !socket.exists(), Duration::from_secs(5));
    c.ok(
        "and the socket is unlinked on the way out",
        !socket.exists(),
    );

    let _ = std::fs::remove_dir_all(&root);
    c.finish();
}

/// Run the real `dreamd approve`, feeding it `payload` on stdin.
fn approve(exe: &Path, socket: &Path, payload: String) -> Value {
    let mut child = Command::new(exe)
        .args(["approve", "--socket"])
        .arg(socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn dreamd approve");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write the hook payload");
    let out = child.wait_with_output().expect("wait");
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn payload(id: &str, tool: &str, input: &str) -> String {
    // The shape captured from claude 2.1.220, fields and all.
    format!(
        r#"{{"session_id":"s","transcript_path":"/t.jsonl","cwd":"/w",
            "permission_mode":"bypassPermissions","hook_event_name":"PreToolUse",
            "tool_name":"{tool}","tool_input":{input},"tool_use_id":"{id}"}}"#
    )
}

fn decision(out: &Value) -> String {
    out.get("hookSpecificOutput")
        .and_then(|o| o.get("permissionDecision"))
        .and_then(|d| d.as_str())
        .unwrap_or("<none>")
        .to_string()
}

fn card_tool(cards: &Arc<Mutex<Vec<Ask>>>, id: &str) -> Option<String> {
    cards
        .lock()
        .unwrap()
        .iter()
        .find(|a| a.id == id)
        .map(|a| a.tool.clone())
}

/// Answer the card with this id as soon as it appears.
fn answer_when_asked(gate: &Arc<Gate>, id: &str, verdict: Verdict, always: bool) {
    let gate = gate.clone();
    let id = id.to_string();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if gate.answer(&id, verdict, always) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
}

fn wait_until(mut cond: impl FnMut() -> bool, within: Duration) {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline && !cond() {
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The `dreamd` binary beside this example.
///
/// `current_exe()` is `target/<profile>/examples/agent_check`; the binary is two
/// directories up. Resolved rather than assumed so the harness works under
/// `--release` and under a custom target directory.
fn dreamd_binary() -> PathBuf {
    let me = std::env::current_exe().expect("current_exe");
    let profile_dir = me
        .parent()
        .and_then(|examples| examples.parent())
        .expect("target/<profile>/examples/<me>");
    let exe = profile_dir.join(if cfg!(windows) {
        "dreamd.exe"
    } else {
        "dreamd"
    });
    assert!(
        exe.exists(),
        "no dreamd binary at {} — run `cargo build` first",
        exe.display()
    );
    exe
}

#[derive(Default)]
struct Checks {
    passed: usize,
    failed: usize,
}

impl Checks {
    fn ok(&mut self, what: &str, cond: bool) {
        if cond {
            self.passed += 1;
        } else {
            self.failed += 1;
            println!("FAIL  {what}");
        }
    }

    fn eq<A, B>(&mut self, what: &str, got: A, want: B)
    where
        A: std::fmt::Debug + PartialEq<B>,
        B: std::fmt::Debug,
    {
        if got == want {
            self.passed += 1;
        } else {
            self.failed += 1;
            println!("FAIL  {what}\n        got  {got:?}\n        want {want:?}");
        }
    }

    fn finish(self) {
        println!(
            "agent_check: {} passed, {} failed",
            self.passed, self.failed
        );
        if self.failed > 0 {
            std::process::exit(1);
        }
    }
}
