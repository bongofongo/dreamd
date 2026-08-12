//! Who decides whether a tool call runs, and how the reader gets asked.
//!
//! The pane's answer to permissions was an argument list: seven tools
//! pre-granted and everything else left to Claude Code's own prompt, drawn in a
//! terminal the reader may not be looking at. A prompt nobody sees is a send
//! that appears to have gone nowhere, which is why the seven were pre-granted in
//! the first place — and why nothing that writes could ever be added to them.
//!
//! This module is the other half. `--permission-prompt-tool` was removed from
//! the CLI before 2.1.220, so the only programmatic gate left is a **`PreToolUse`
//! hook**, and a hook turns out to be a much better one than the flag was:
//! measured against 2.1.220, it fires **even under `--permission-mode
//! bypassPermissions`** and its verdict stands. The gate outranks the mode.
//!
//! That inverts the old trade. The allowlist stops being the whole policy and
//! becomes the *fast path* — a tool on it never reaches the hook at all, so the
//! common case costs no process spawn — while everything else raises a card in
//! the window the reader is already looking at. The policy itself moves from an
//! argv string into [`decide`], which is a pure function with tests.
//!
//! # Deny is the answer to every kind of silence
//!
//! A closed pane, a window that never answered, a socket whose owner has gone,
//! a hook that outlived its timeout: all of them are [`Verdict::Deny`]. There is
//! no path here that fails open, and the tests say so in those words. A denial
//! is cheap — the agent is told no, says so, and carries on — while the failure
//! in the other direction is a write the reader never agreed to.
//!
//! # What "always" means, and what it does not
//!
//! The card's third button adds a tool to a set that lives in this struct and
//! dies with the process. It is not written to `config.toml` and it is not a
//! `permissions` entry in Claude Code's settings. A reader who says "always" is
//! answering for **this conversation**, and a preference that outlived the
//! conversation would be a durable grant made in a moment of impatience.

use std::collections::HashSet;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::claude::GRANTS;

/// How long a hook waits on the reader before the answer becomes no.
///
/// Two minutes is long enough to read a card, think, and click, and short
/// enough that a forgotten window does not leave a `claude` process parked on a
/// blocked hook indefinitely. It must stay **under** the `timeout` dreamd
/// declares for the hook itself, or the CLI gives up first and the reader's
/// click lands on nothing.
pub const WAIT: Duration = Duration::from_secs(120);

/// The hook timeout dreamd declares, in seconds. Comfortably past [`WAIT`], so
/// the gate is always the thing that decides.
pub const HOOK_TIMEOUT_SECS: u64 = 150;

/// What a tool call is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Allow,
    Deny,
}

/// What [`decide`] concluded without asking anybody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// On the allowlist, or granted for this session. Runs; no card.
    Auto,
    /// Needs the reader. Raise a card and wait.
    Ask,
}

/// The standing policy for a tool name, before the reader is troubled.
///
/// Pure, and the whole of the rule: on the list is [`Policy::Auto`], everything
/// else is [`Policy::Ask`]. `always` is the session's accumulated answers.
///
/// Matching is exact. Claude Code's own allowlist syntax has specifiers —
/// `Bash(git *)` — and dreamd deliberately does not implement them: a prefix
/// rule is a small language, and a small language for deciding what may run on
/// the reader's machine is a place to be wrong quietly. A card names the exact
/// call.
pub fn decide(tool_name: &str, always: &HashSet<String>) -> Policy {
    if GRANTS.contains(&tool_name) || always.contains(tool_name) {
        Policy::Auto
    } else {
        Policy::Ask
    }
}

/// One tool call waiting on the reader.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Ask {
    /// Claude Code's `tool_use_id`. The card's identity and the frontend's
    /// handle on it.
    pub id: String,
    pub tool: String,
    /// The call's arguments, verbatim, for the card to render. Not delimited:
    /// this is shown to the *reader*, who is the authority here, not passed to
    /// an agent.
    pub input: serde_json::Value,
}

/// Where a card goes. The same closure shape as [`super::Sink`], for the same
/// reason — no `AppHandle` in this module, so `agent_check` can drive the whole
/// gate with no window.
pub type Asker = Arc<dyn Fn(Ask) + Send + Sync>;

/// One outstanding card, and the hook thread parked on its answer.
struct Waiting {
    id: String,
    /// Carried here rather than sent back with the answer, so that "always"
    /// grants the tool that was actually asked about — a frontend that named a
    /// different one could otherwise widen the session's policy by a typo.
    tool: String,
    reply: mpsc::Sender<Verdict>,
}

/// The pending questions and the session's standing answers.
pub struct Gate {
    ask: Asker,
    always: Mutex<HashSet<String>>,
    waiting: Mutex<Vec<Waiting>>,
}

impl Gate {
    pub fn new(ask: Asker) -> Self {
        Self {
            ask,
            always: Mutex::new(HashSet::new()),
            waiting: Mutex::new(Vec::new()),
        }
    }

    /// Rule on one tool call, blocking until the reader answers or [`WAIT`]
    /// elapses.
    ///
    /// Called from a socket thread, one per hook process, so blocking here
    /// blocks exactly the tool call it is about.
    pub fn request(&self, id: &str, tool: &str, input: serde_json::Value) -> Verdict {
        if decide(tool, &self.always.lock().unwrap()) == Policy::Auto {
            return Verdict::Allow;
        }

        let (reply, rx) = mpsc::channel();
        self.waiting.lock().unwrap().push(Waiting {
            id: id.to_string(),
            tool: tool.to_string(),
            reply,
        });

        (self.ask)(Ask {
            id: id.to_string(),
            tool: tool.to_string(),
            input,
        });

        let verdict = match rx.recv_timeout(WAIT) {
            Ok(v) => v,
            // Timed out, or the window dropped the sender by going away. Both
            // are silence, and silence is no.
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => Verdict::Deny,
        };
        self.forget(id);
        verdict
    }

    /// The reader's answer. `always` records the tool for the rest of the
    /// session; see the module docs for why that is all it records.
    ///
    /// Returns false if the id named nothing — a card clicked twice, or one
    /// whose hook already timed out and went away.
    pub fn answer(&self, id: &str, verdict: Verdict, always: bool) -> bool {
        let entry = {
            let mut waiting = self.waiting.lock().unwrap();
            match waiting.iter().position(|w| w.id == id) {
                Some(i) => waiting.remove(i),
                None => return false,
            }
        };
        // Only an allow can become standing. "Always deny" would be a rule the
        // reader could not see, could not list and could not take back.
        if always && verdict == Verdict::Allow {
            self.always.lock().unwrap().insert(entry.tool);
        }
        entry.reply.send(verdict).is_ok()
    }

    /// Deny everything outstanding. Called when the pane closes or the session
    /// ends: a card with no window to be clicked in is silence.
    pub fn close(&self) {
        for entry in self.waiting.lock().unwrap().drain(..) {
            let _ = entry.reply.send(Verdict::Deny);
        }
    }

    /// How many cards are outstanding. For the harness and the pane's own
    /// bookkeeping.
    pub fn outstanding(&self) -> usize {
        self.waiting.lock().unwrap().len()
    }

    /// Whether a tool has a standing grant for this session.
    pub fn granted(&self, tool: &str) -> bool {
        self.always.lock().unwrap().contains(tool)
    }

    fn forget(&self, id: &str) {
        self.waiting.lock().unwrap().retain(|w| w.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn silent() -> Gate {
        Gate::new(Arc::new(|_| {}))
    }

    #[test]
    fn the_pre_granted_tools_never_raise_a_card() {
        let always = HashSet::new();
        for tool in GRANTS {
            assert_eq!(decide(tool, &always), Policy::Auto, "{tool} should be auto");
        }
    }

    #[test]
    fn everything_else_asks() {
        let always = HashSet::new();
        for tool in ["Bash", "Edit", "Write", "WebFetch", "Task", "NotebookEdit"] {
            assert_eq!(decide(tool, &always), Policy::Ask, "{tool} should ask");
        }
    }

    #[test]
    fn a_near_miss_on_a_granted_name_still_asks() {
        // Exact matching, stated as a property: no prefix rule, no specifier
        // syntax, nothing that could make `Read` cover `ReadAndDelete`.
        let always = HashSet::new();
        for tool in [
            "read",
            "READ",
            "Read ",
            "ReadFile",
            "Readme",
            "mcp__dreamd__get_stack_and_more",
            "mcp__other__get_stack",
            "mcp__dreamd__*",
            "*",
        ] {
            assert_eq!(decide(tool, &always), Policy::Ask, "{tool} must not pass");
        }
    }

    #[test]
    fn a_session_grant_makes_a_tool_auto_and_nothing_else() {
        let mut always = HashSet::new();
        always.insert("Bash".to_string());
        assert_eq!(decide("Bash", &always), Policy::Auto);
        assert_eq!(decide("Edit", &always), Policy::Ask);
    }

    #[test]
    fn an_auto_tool_is_allowed_without_troubling_the_reader() {
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = asked.clone();
        let gate = Gate::new(Arc::new(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        }));
        let v = gate.request("t1", "Read", serde_json::json!({"file_path": "/w/a.md"}));
        assert_eq!(v, Verdict::Allow);
        assert_eq!(asked.load(Ordering::Relaxed), 0, "no card for a grant");
    }

    #[test]
    fn a_card_the_reader_allows_allows() {
        let gate = Arc::new(silent());
        let g = gate.clone();
        std::thread::spawn(move || {
            while g.outstanding() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            g.answer("t2", Verdict::Allow, false);
        });
        assert_eq!(
            gate.request("t2", "Bash", serde_json::json!({"command": "ls"})),
            Verdict::Allow
        );
        assert_eq!(gate.outstanding(), 0, "the card is gone once answered");
    }

    #[test]
    fn a_card_the_reader_denies_denies() {
        let gate = Arc::new(silent());
        let g = gate.clone();
        std::thread::spawn(move || {
            while g.outstanding() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            g.answer("t3", Verdict::Deny, false);
        });
        assert_eq!(
            gate.request("t3", "Bash", serde_json::json!({"command": "rm -rf /"})),
            Verdict::Deny
        );
    }

    #[test]
    fn a_closed_pane_denies_what_was_waiting() {
        // The window going away is silence, and silence is no.
        let gate = Arc::new(silent());
        let g = gate.clone();
        std::thread::spawn(move || {
            while g.outstanding() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            g.close();
        });
        assert_eq!(
            gate.request("t4", "Write", serde_json::json!({"file_path": "/w/a.md"})),
            Verdict::Deny
        );
    }

    #[test]
    fn answering_an_unknown_card_changes_nothing() {
        let gate = silent();
        assert!(!gate.answer("never-asked", Verdict::Allow, false));
        assert!(!gate.answer("never-asked", Verdict::Allow, true));
        assert!(
            !gate.granted("Bash"),
            "an answer to nothing must not grant anything"
        );
    }

    #[test]
    fn a_card_cannot_be_answered_twice() {
        let gate = Arc::new(silent());
        let g = gate.clone();
        let second = std::thread::spawn(move || {
            while g.outstanding() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            let first = g.answer("t5", Verdict::Deny, false);
            let again = g.answer("t5", Verdict::Allow, false);
            (first, again)
        });
        assert_eq!(
            gate.request("t5", "Bash", serde_json::json!({})),
            Verdict::Deny
        );
        let (first, again) = second.join().unwrap();
        assert!(first, "the first answer lands");
        assert!(!again, "the second finds nothing to answer");
    }

    #[test]
    fn always_grants_the_tool_that_was_asked_about() {
        let gate = Arc::new(silent());
        let g = gate.clone();
        std::thread::spawn(move || {
            while g.outstanding() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            g.answer("t7", Verdict::Allow, true);
        });
        assert_eq!(
            gate.request("t7", "Bash", serde_json::json!({"command": "ls"})),
            Verdict::Allow
        );
        assert!(gate.granted("Bash"), "the grant is recorded");
        assert!(!gate.granted("Edit"), "and only for that tool");

        // Which is the whole point of a standing grant: the next one is free.
        // Nobody is answering this one, so a card would park it until `WAIT`
        // elapsed — returning at all is the assertion that none was raised.
        assert_eq!(
            gate.request("t8", "Bash", serde_json::json!({"command": "pwd"})),
            Verdict::Allow
        );
    }

    #[test]
    fn always_deny_is_not_a_thing_a_reader_can_create() {
        // A standing denial would be a rule with no surface to see it in and no
        // gesture to take it back.
        let gate = Arc::new(silent());
        let g = gate.clone();
        std::thread::spawn(move || {
            while g.outstanding() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            g.answer("t9", Verdict::Deny, true);
        });
        assert_eq!(
            gate.request("t9", "Bash", serde_json::json!({})),
            Verdict::Deny
        );
        assert!(!gate.granted("Bash"));
    }

    #[test]
    fn the_hook_timeout_outlives_the_gates_wait() {
        // If these crossed, the CLI would give up before the reader could
        // click, and the click would land on a hook that had already gone.
        assert!(Duration::from_secs(HOOK_TIMEOUT_SECS) > WAIT);
    }

    #[test]
    fn the_card_carries_the_call_it_is_about() {
        let seen = Arc::new(Mutex::new(None));
        let sink = seen.clone();
        let gate = Arc::new(Gate::new(Arc::new(move |ask| {
            *sink.lock().unwrap() = Some(ask);
        })));
        let g = gate.clone();
        std::thread::spawn(move || {
            while g.outstanding() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            g.answer("t6", Verdict::Deny, false);
        });
        let _ = gate.request("t6", "Bash", serde_json::json!({"command": "cargo build"}));
        let ask = seen.lock().unwrap().clone().expect("a card was raised");
        assert_eq!(ask.id, "t6");
        assert_eq!(ask.tool, "Bash");
        assert_eq!(ask.input["command"], "cargo build");
    }
}
