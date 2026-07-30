//! Claude Code's `stream-json` output, turned into the handful of things a
//! reader's window actually shows.
//!
//! This is the whole of dreamd's knowledge of another program's wire format,
//! deliberately concentrated in one pure function so the drift has one place to
//! land. Everything here is `serde_json::Value` navigation rather than derived
//! structs, and that is the design rather than laziness: a `Deserialize` impl
//! decides in advance what a message may contain, and this format is one we do
//! not own and did not version. `--forward-subagent-text` and
//! `--include-hook-events` were both added to the CLI after the shapes below
//! were captured, and the next release will add more.
//!
//! **So the contract is leniency, and it is load-bearing.** [`digest`] returns a
//! `Vec`, never a `Result`: an unrecognised `type`, an unrecognised
//! `content_block`, a line that is not JSON at all — each yields an empty
//! vector. A message kind dreamd has never seen costs a missing ticker row. It
//! must never cost the pane, because the pane is where the answer to the
//! reader's question is arriving. The `edges.ndjson` fixture carries a
//! made-up top-level `type`, a made-up `stream_event` event type and a line of
//! prose precisely so a change here that turns any of them fatal goes red.
//!
//! # The two paths text arrives by, and why both are read
//!
//! With `--include-partial-messages` a text block arrives twice: once as a run
//! of `content_block_delta`/`text_delta` while the model is producing it, and
//! again, whole, in the `type: "assistant"` message that closes it. dreamd
//! consumes both, and the split is what the pane's two-stage rendering is made
//! of — [`AgentEvent::TextDelta`] appends plain text as it lands, so the reader
//! sees prose move; [`AgentEvent::Text`] then replaces that block wholesale with
//! the settled string, which is the one that goes through `markdown::to_html`.
//! Streaming plain, settled typeset. Blocks are keyed by `index`, which is the
//! only identifier the delta events carry.
//!
//! Tool calls are read from the **complete** `assistant` message and not from
//! the stream, because `content_block_start` announces a `tool_use` with an
//! empty `input` and dribbles the real one out as `input_json_delta` fragments
//! that are only valid JSON once concatenated. The ticker wants a target to
//! print, not a parser to run.
//!
//! # What is deliberately dropped
//!
//! - **Thinking.** v1 shows text and a tool ticker; a reasoning trace is
//!   neither, and `system/status` already answers "is it working".
//! - **Anything with a `parent_tool_use_id`.** That is a sub-agent's chatter,
//!   surfaced under `--forward-subagent-text`. It belongs to a Task the reader
//!   did not ask a question about, and threading it into the same log would
//!   read as the agent contradicting itself.
//! - **Hook events, rate-limit events, `message_start`/`message_stop`.** Noise
//!   at this altitude.

use serde::Serialize;

/// One thing worth showing the reader.
///
/// Serialized straight to the frontend as the payload of an `agent-event`, so
/// the variant names are part of `ui/app.js`'s contract.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AgentEvent {
    /// A session announced itself. Emitted **once per turn**, not once per
    /// process — the CLI re-emits `system/init` each time it comes back round —
    /// so the frontend must treat this as "still here, this is the model now"
    /// rather than "start a new conversation". That is exactly what makes the
    /// `/model` chips work without a restart.
    Ready { session_id: String, model: String },
    /// The CLI's own working indicator (`requesting`, and whatever it adds).
    Status { status: String },
    /// Provisional text, as it streams. Append to block `index`.
    TextDelta { index: u64, text: String },
    /// The settled text of block `index`. Replaces whatever the deltas built.
    Text { index: u64, text: String },
    /// The CLI answering for itself rather than the model — a `/model`
    /// acknowledgement is the one that matters here. Rendered as a note, not as
    /// the agent's prose, because attributing it to the agent would be a small
    /// lie in the transcript the reader keeps.
    Notice { text: String },
    /// A tool call started. `target` is the one field worth putting in a
    /// one-line ticker; see [`target`].
    ToolStart {
        id: String,
        name: String,
        target: Option<String>,
    },
    /// A tool call finished. `ok` is false for a genuine failure *and* for a
    /// refusal — from the ticker's point of view they are the same row, and the
    /// card that caused a refusal has already said so in its own words.
    ToolEnd { id: String, ok: bool },
    /// The turn ended. `ok` is false when the CLI reports `is_error`, which
    /// covers both a failure and an interrupt.
    Turn {
        ok: bool,
        interrupted: bool,
        cost_usd: f64,
        duration_ms: u64,
        denials: usize,
    },
}

/// Turn one NDJSON line into zero or more events.
///
/// Zero is the normal answer for most lines. It is also the answer for garbage,
/// on purpose — see the module docs.
pub fn digest(line: &str) -> Vec<AgentEvent> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };

    // A sub-agent's output carries the id of the Task that spawned it. Not this
    // conversation; see the module docs.
    if v.get("parent_tool_use_id")
        .and_then(|p| p.as_str())
        .is_some()
    {
        return Vec::new();
    }

    match str_at(&v, "type") {
        Some("system") => system(&v),
        Some("stream_event") => stream_event(&v),
        Some("assistant") => assistant(&v),
        Some("user") => user(&v),
        Some("result") => result(&v),
        _ => Vec::new(),
    }
}

fn system(v: &serde_json::Value) -> Vec<AgentEvent> {
    match str_at(v, "subtype") {
        Some("init") => {
            let session_id = str_at(v, "session_id").unwrap_or_default().to_string();
            let model = str_at(v, "model").unwrap_or_default().to_string();
            vec![AgentEvent::Ready { session_id, model }]
        }
        Some("status") => str_at(v, "status")
            .map(|s| {
                vec![AgentEvent::Status {
                    status: s.to_string(),
                }]
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn stream_event(v: &serde_json::Value) -> Vec<AgentEvent> {
    let Some(event) = v.get("event") else {
        return Vec::new();
    };
    // Only text deltas. `input_json_delta` fragments are ignored because the
    // whole input arrives in the closing `assistant` message anyway.
    if str_at(event, "type") != Some("content_block_delta") {
        return Vec::new();
    }
    let Some(delta) = event.get("delta") else {
        return Vec::new();
    };
    if str_at(delta, "type") != Some("text_delta") {
        return Vec::new();
    }
    let Some(text) = str_at(delta, "text") else {
        return Vec::new();
    };
    vec![AgentEvent::TextDelta {
        index: event.get("index").and_then(|i| i.as_u64()).unwrap_or(0),
        text: text.to_string(),
    }]
}

fn assistant(v: &serde_json::Value) -> Vec<AgentEvent> {
    let Some(message) = v.get("message") else {
        return Vec::new();
    };
    // The CLI answers some things itself — `/model`, and whatever it adds later
    // — under a reserved model name. Those are dreamd's notices, not the
    // agent's words.
    let synthetic = str_at(message, "model") == Some(SYNTHETIC_MODEL);
    let Some(blocks) = message.get("content").and_then(|c| c.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        match str_at(block, "type") {
            Some("text") => {
                let Some(text) = str_at(block, "text") else {
                    continue;
                };
                out.push(if synthetic {
                    AgentEvent::Notice {
                        text: text.to_string(),
                    }
                } else {
                    AgentEvent::Text {
                        index: index as u64,
                        text: text.to_string(),
                    }
                });
            }
            Some("tool_use") => {
                let (Some(id), Some(name)) = (str_at(block, "id"), str_at(block, "name")) else {
                    continue;
                };
                out.push(AgentEvent::ToolStart {
                    id: id.to_string(),
                    name: name.to_string(),
                    target: target(name, block.get("input")),
                });
            }
            // Thinking and anything newer: dropped.
            _ => {}
        }
    }
    out
}

fn user(v: &serde_json::Value) -> Vec<AgentEvent> {
    let Some(blocks) = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|b| str_at(b, "type") == Some("tool_result"))
        .filter_map(|b| {
            str_at(b, "tool_use_id").map(|id| AgentEvent::ToolEnd {
                id: id.to_string(),
                // Absent means fine. A refusal and a failure both set it.
                ok: !b.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false),
            })
        })
        .collect()
}

fn result(v: &serde_json::Value) -> Vec<AgentEvent> {
    let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
    vec![AgentEvent::Turn {
        ok: !is_error,
        interrupted: str_at(v, "terminal_reason") == Some("interrupted"),
        cost_usd: v
            .get("total_cost_usd")
            .and_then(|c| c.as_f64())
            .unwrap_or(0.0),
        duration_ms: v.get("duration_ms").and_then(|d| d.as_u64()).unwrap_or(0),
        denials: v
            .get("permission_denials")
            .and_then(|d| d.as_array())
            .map(|a| a.len())
            .unwrap_or(0),
    }]
}

/// The reserved model name the CLI uses for its own replies.
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// How much of a tool's target a ticker row will show before it stops being a
/// row.
const TARGET_MAX: usize = 80;

/// The input fields worth printing beside a tool name, most specific first.
///
/// Ordered rather than matched per tool on purpose: a tool dreamd has never
/// heard of still gets a useful row if it names its subject conventionally, and
/// the alternative — a table keyed by tool name — would be a second thing to
/// keep in step with a program we do not ship.
const TARGET_KEYS: [&str; 7] = [
    "file_path",
    "path",
    "command",
    "pattern",
    "url",
    "query",
    "description",
];

/// Pick the one field worth showing beside `name` in a one-line ticker.
///
/// Whitespace is collapsed because a heredoc in a `Bash` command would
/// otherwise make one row as tall as the pane.
pub fn target(_name: &str, input: Option<&serde_json::Value>) -> Option<String> {
    let input = input?;
    let raw = TARGET_KEYS
        .iter()
        .find_map(|k| input.get(k).and_then(|v| v.as_str()))?;

    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return None;
    }
    Some(if flat.chars().count() > TARGET_MAX {
        let cut: String = flat.chars().take(TARGET_MAX - 1).collect();
        format!("{cut}…")
    } else {
        flat
    })
}

fn str_at<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|k| k.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TURN: &str = include_str!("fixtures/turn.ndjson");
    const EDGES: &str = include_str!("fixtures/edges.ndjson");

    fn all(fixture: &str) -> Vec<AgentEvent> {
        fixture.lines().flat_map(digest).collect()
    }

    #[test]
    fn a_whole_turn_reduces_to_the_events_the_pane_draws() {
        let events = all(TURN);
        assert_eq!(
            events,
            vec![
                AgentEvent::Ready {
                    session_id: "s1".into(),
                    model: "claude-haiku-4-5-20251001".into(),
                },
                AgentEvent::Status {
                    status: "requesting".into()
                },
                AgentEvent::ToolStart {
                    id: "toolu_A".into(),
                    name: "Read".into(),
                    target: Some("/w/notes/sample.md".into()),
                },
                AgentEvent::ToolEnd {
                    id: "toolu_A".into(),
                    ok: true
                },
                AgentEvent::Status {
                    status: "requesting".into()
                },
                AgentEvent::TextDelta {
                    index: 0,
                    text: "File says **hello** ".into()
                },
                AgentEvent::TextDelta {
                    index: 0,
                    text: "from fixture.".into()
                },
                AgentEvent::Text {
                    index: 0,
                    text: "File says **hello** from fixture.".into()
                },
                AgentEvent::Turn {
                    ok: true,
                    interrupted: false,
                    cost_usd: 0.0199866,
                    duration_ms: 3627,
                    denials: 0,
                },
            ]
        );
    }

    #[test]
    fn the_deltas_concatenate_to_the_settled_text() {
        // The two-stage rendering only works if this holds: the pane appends
        // deltas, then replaces the block with `Text`. If they disagreed the
        // prose would visibly rewrite itself at the end of every block.
        let events = all(TURN);
        let joined: String = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { index: 0, text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let settled = events.iter().find_map(|e| match e {
            AgentEvent::Text { index: 0, text } => Some(text.clone()),
            _ => None,
        });
        assert_eq!(Some(joined), settled);
    }

    #[test]
    fn nothing_in_a_turn_is_fatal_and_nothing_unknown_is_emitted() {
        // Every line of both fixtures, one at a time. The assertion is that
        // `digest` returns — the leniency contract in its bluntest form.
        for line in TURN.lines().chain(EDGES.lines()) {
            let _ = digest(line);
        }
    }

    #[test]
    fn unknown_kinds_and_outright_garbage_yield_nothing() {
        for line in [
            r#"{"type":"some_message_kind_from_a_later_release","payload":{}}"#,
            r#"{"type":"stream_event","event":{"type":"a_new_event_type","index":0}}"#,
            r#"{"type":"system","subtype":"hook_started","hook_id":"h1"}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{}}"#,
            "this line is not JSON at all",
            "{",
            "",
            "   ",
            "null",
            "[]",
            r#""a bare string""#,
        ] {
            assert_eq!(digest(line), Vec::new(), "should ignore {line:?}");
        }
    }

    #[test]
    fn a_refusal_and_a_failure_are_both_a_crossed_row() {
        let events = all(EDGES);
        assert!(events.contains(&AgentEvent::ToolEnd {
            id: "toolu_B".into(),
            ok: false
        }));
        // And the block-array form of `content` still reports success.
        assert!(events.contains(&AgentEvent::ToolEnd {
            id: "toolu_C".into(),
            ok: true
        }));
    }

    #[test]
    fn the_cli_answering_for_itself_is_a_notice_not_the_agents_words() {
        // `/model haiku` comes back as an assistant message under a reserved
        // model name. Rendering it as prose would put words in the agent's
        // mouth in the transcript the reader keeps.
        let events = all(EDGES);
        assert!(events.contains(&AgentEvent::Notice {
            text: "Set model to Haiku 4.5 for this session only".into()
        }));
        assert!(
            !events.iter().any(
                |e| matches!(e, AgentEvent::Text { text, .. } if text.starts_with("Set model"))
            ),
            "a synthetic reply must not also arrive as agent prose"
        );
    }

    #[test]
    fn subagent_chatter_is_not_this_conversation() {
        let events = all(EDGES);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::Text { text, .. } if text.contains("Sub-agent"))),
            "a parent_tool_use_id means someone else's turn"
        );
    }

    #[test]
    fn an_interrupted_turn_says_so() {
        let turn = all(EDGES)
            .into_iter()
            .find_map(|e| match e {
                AgentEvent::Turn {
                    ok, interrupted, ..
                } => Some((ok, interrupted)),
                _ => None,
            })
            .expect("the fixture ends in a result");
        assert_eq!(turn, (false, true));
    }

    #[test]
    fn denials_are_counted_so_the_pane_can_say_what_it_blocked() {
        let denials = all(EDGES)
            .into_iter()
            .find_map(|e| match e {
                AgentEvent::Turn { denials, .. } => Some(denials),
                _ => None,
            })
            .expect("the fixture ends in a result");
        assert_eq!(denials, 1);
    }

    #[test]
    fn a_target_is_picked_by_convention_not_by_tool_name() {
        let input = serde_json::json!({"pattern": "locate_near", "path": "src"});
        // `path` outranks `pattern`, and a tool nobody has heard of still gets a row.
        assert_eq!(
            target("SomeFutureTool", Some(&input)).as_deref(),
            Some("src")
        );
        assert_eq!(target("Read", None), None);
        assert_eq!(target("Read", Some(&serde_json::json!({}))), None);
    }

    #[test]
    fn a_multiline_target_stays_one_row() {
        let input =
            serde_json::json!({"command": "cargo build \\\n  --release\n\n# and a comment"});
        let got = target("Bash", Some(&input)).expect("a command is a target");
        assert!(!got.contains('\n'), "got {got:?}");
        assert!(got.chars().count() <= TARGET_MAX, "got {got:?}");
    }

    #[test]
    fn a_long_target_is_cut_to_one_row() {
        let long = "a".repeat(500);
        let input = serde_json::json!({ "file_path": long });
        let got = target("Read", Some(&input)).expect("a path is a target");
        assert_eq!(got.chars().count(), TARGET_MAX);
        assert!(got.ends_with('…'));
    }

    #[test]
    fn an_empty_target_is_no_target_rather_than_an_empty_row() {
        let input = serde_json::json!({"command": "   \n  "});
        assert_eq!(target("Bash", Some(&input)), None);
    }
}
