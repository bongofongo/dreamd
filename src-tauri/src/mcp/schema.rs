//! The tool list, compiled in as one `const &str` of JSON.
//!
//! Both halves of the binary read this const: the GUI's in-process server, and
//! the `dreamd mcp` stdio shim, which answers `initialize` and `tools/list`
//! locally instead of proxying them. That is load-bearing — if the shim proxied
//! `tools/list` and dreamd happened to be closed when Claude Code started, the
//! client would cache an empty tool list for the whole session. One const, used
//! by both, means zero drift.
//!
//! **The descriptions below are product surface, not boilerplate.** They are
//! the only thing steering an agent toward the human's queue rather than a
//! sweep of the repo, and the reader is a model, so they are written for one.
//! Getting them wrong fails no test; it produces an agent that technically
//! works and behaves wrongly. `get_stack` says it is the entry point and that
//! the order is the human's asking order. `list_highlights` says, in as many
//! words, that it is *not* how you find work. Treat edits here as copy changes
//! and review them as such.

use serde_json::Value;

/// The MCP revision dreamd speaks. A `const` so step 3c's shim and the GUI
/// server report the same thing; bumping it is a deliberate one-line change.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

pub const SERVER_NAME: &str = "dreamd";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Shown to the model once, at `initialize`, before it has called anything.
/// This is where the queue-first framing gets stated as a whole rather than
/// tool by tool.
pub const INSTRUCTIONS: &str =
    "dreamd is the human's reading window: they read markdown, highlight passages, \
     and attach a question or comment to the ones they want you to deal with. \
     Those annotated passages form an ordered queue, and that queue is what \
     this server exists to hand you.\n\n\
     Start with get_stack. It returns the open questions in the order the human \
     asked them, each with the exact passage it is about. Work through them in \
     that order, using your own tools to read code, search and edit, and call \
     resolve_highlight on each one as you finish it — the dreamd window updates \
     live, so that is how the human follows along without touching the app.\n\n\
     Do not go looking for work by listing marks across files. If it is not on \
     the queue, nobody asked for it. dreamd never writes to the user's markdown \
     and neither does this server: every file change is yours to make with your \
     own tools.";

/// The tool list, verbatim as `tools/list` returns it.
///
/// A `const &str` rather than a built `Value` so the exact bytes the shim
/// serves are the exact bytes in the source, and so the drift test in
/// `tools.rs` has something to parse.
pub const TOOLS: &str = r#"[
  {
    "name": "get_stack",
    "description": "Start here. Returns the human's queue of open questions: each passage they highlighted in dreamd together with the note they attached to it, in the order they asked. This queue is the work. It is what the human built by reading and annotating, and handing it to you is dreamd's whole purpose, so working through it in order is the expected behaviour. Every entry arrives with its quote attached, so you can usually begin without reading anything else first. Do not sweep files looking for things to do: if it is not on this queue, nobody asked for it. Call resolve_highlight on each entry as you finish it.",
    "inputSchema": {
      "type": "object",
      "properties": {},
      "additionalProperties": false
    },
    "annotations": {
      "title": "Get the human's queue of open questions",
      "readOnlyHint": true,
      "destructiveHint": false,
      "idempotentHint": true,
      "openWorldHint": false
    }
  },
  {
    "name": "get_open_document",
    "description": "Returns the repo-relative path of the document the human currently has open in dreamd, or nothing if they have none. Reach for it when what they said depends on where they are: a question that says this file, this section or here, and a queue entry is not what is being pointed at. It answers with a path and nothing else, so read the file with your own tools once you have it. This is not a way to find work: the human's questions come from get_stack, and a document being open is not a request to do anything to it.",
    "inputSchema": {
      "type": "object",
      "properties": {},
      "additionalProperties": false
    },
    "annotations": {
      "title": "Get the document the human is reading",
      "readOnlyHint": true,
      "destructiveHint": false,
      "idempotentHint": true,
      "openWorldHint": false
    }
  },
  {
    "name": "get_highlight",
    "description": "Fetch one mark by id together with the text immediately before and after it, which get_stack leaves out for brevity. Use it when the quote alone is too little to act on: a fragment whose subject sits in the previous sentence, a list item whose heading is what makes it mean anything. It returns anchoring context only, never the file's contents. If you need more than that, read the file with your own tools.",
    "inputSchema": {
      "type": "object",
      "properties": {
        "id": {
          "type": "string",
          "description": "The mark id, as returned by get_stack, list_highlights or mark_passage."
        }
      },
      "required": ["id"],
      "additionalProperties": false
    },
    "annotations": {
      "title": "Get one mark with its surrounding context",
      "readOnlyHint": true,
      "destructiveHint": false,
      "idempotentHint": true,
      "openWorldHint": false
    }
  },
  {
    "name": "resolve_highlight",
    "description": "Close one of the human's open questions once you have dealt with it, with a short note saying what you did or found. The entry leaves the queue and the dreamd window repaints immediately, so this is how the human watches progress without touching the app: call it as you finish each item rather than batching them all at the end. The highlight itself is kept, so the passage and your note stay anchored to the text as a record of the exchange. Returns how many questions are still on the queue.",
    "inputSchema": {
      "type": "object",
      "properties": {
        "id": {
          "type": "string",
          "description": "The mark id to close."
        },
        "note": {
          "type": "string",
          "description": "What you did or found, in a sentence or two. Written for the human, who will read it beside the passage it answers."
        }
      },
      "required": ["id"],
      "additionalProperties": false
    },
    "annotations": {
      "title": "Close a question on the queue",
      "readOnlyHint": false,
      "destructiveHint": false,
      "idempotentHint": true,
      "openWorldHint": false
    }
  },
  {
    "name": "list_highlights",
    "description": "List the marks in a file you are already working in. This is not how you find work: the human's queue is get_stack, and starting here will have you reading documents for things nobody asked about. Reach for it when you are already editing a file and want to see what is flagged in it, or when auditing marks that went stale because the text beneath them changed. Filter by file, by state, and by whether they have been resolved.",
    "inputSchema": {
      "type": "object",
      "properties": {
        "file": {
          "type": "string",
          "description": "Repo-relative path, e.g. docs/plan.md. Omit for every file."
        },
        "state": {
          "type": "string",
          "enum": ["active", "stale"],
          "description": "active: the mark still matches the text it was anchored to. stale: the text beneath it was edited and the anchor no longer resolves."
        },
        "resolved": {
          "type": "boolean",
          "description": "true for marks already closed with resolve_highlight, false for still-open ones. Omit for both."
        }
      },
      "additionalProperties": false
    },
    "annotations": {
      "title": "List marks in a file you are already in",
      "readOnlyHint": true,
      "destructiveHint": false,
      "idempotentHint": true,
      "openWorldHint": false
    }
  },
  {
    "name": "mark_passage",
    "description": "Anchor a note of your own to a passage, so it shows up in the dreamd window as a mark the human can read beside their own. Use it to leave a finding where it will be met in context: a caveat on the paragraph it applies to, an answer next to the sentence that prompted it, rather than only in your reply. The quote must appear in the file verbatim. Your marks do not join the human's queue, and this does not edit the file: dreamd never writes to the user's markdown.",
    "inputSchema": {
      "type": "object",
      "properties": {
        "file": {
          "type": "string",
          "description": "Repo-relative path to a file inside the repo dreamd has open. Paths outside it are refused."
        },
        "quote": {
          "type": "string",
          "description": "The exact text to anchor to, copied verbatim from the file. Keep it to a sentence or two: a shorter quote survives later edits to the rest of the paragraph."
        },
        "note": {
          "type": "string",
          "description": "The note to attach, written for the human reading the document."
        }
      },
      "required": ["file", "quote", "note"],
      "additionalProperties": false
    },
    "annotations": {
      "title": "Leave a note on a passage",
      "readOnlyHint": false,
      "destructiveHint": false,
      "idempotentHint": false,
      "openWorldHint": false
    }
  }
]"#;

/// [`TOOLS`], parsed.
///
/// Panics if the const is not valid JSON, which is a compile-time-shaped bug
/// caught by the first test in this module rather than anything a user can
/// reach.
pub fn tools() -> Value {
    serde_json::from_str(TOOLS).expect("the compiled-in tool schema is valid JSON")
}

/// The tool names in schema order. The half of the drift guard that lives here.
pub fn tool_names() -> Vec<String> {
    tools()
        .as_array()
        .expect("the tool schema is an array")
        .iter()
        .map(|t| {
            t["name"]
                .as_str()
                .expect("every tool has a string name")
                .to_string()
        })
        .collect()
}

/// The `tools/list` result.
pub fn tools_list_result() -> Value {
    serde_json::json!({ "tools": tools() })
}

/// The `initialize` result.
pub fn initialize_result() -> Value {
    serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
        "instructions": INSTRUCTIONS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiled_in_schema_is_valid_json() {
        // The const is hand-written JSON in a raw string, so nothing but a test
        // checks it. A trailing comma here would break `tools/list` for every
        // client at once.
        let tools = tools();
        let arr = tools.as_array().expect("an array");
        assert_eq!(arr.len(), 6, "four read tools and two write tools");
    }

    #[test]
    fn every_tool_declares_a_name_description_and_object_input_schema() {
        for tool in tools().as_array().expect("an array") {
            let name = tool["name"].as_str().expect("a name");
            let desc = tool["description"].as_str().expect("a description");
            assert!(
                desc.len() > 120,
                "{name}'s description is filler, not product surface: {desc:?}"
            );
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            // `additionalProperties: false` is what makes a typo in an argument
            // name an error the model can see rather than a silently ignored
            // field.
            assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
            assert!(
                tool["annotations"]["readOnlyHint"].is_boolean(),
                "{name} must declare whether it writes"
            );
        }
    }

    #[test]
    fn the_read_only_hints_match_the_three_read_two_write_split() {
        let tools = tools();
        let read_only: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["annotations"]["readOnlyHint"] == true)
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            read_only,
            vec![
                "get_stack",
                "get_open_document",
                "get_highlight",
                "list_highlights"
            ]
        );
    }

    #[test]
    fn the_descriptions_point_the_agent_at_the_queue_not_at_the_files() {
        // The single most important thing in this file, and the only assertion
        // that can be made about copy: `get_stack` must claim the entry point
        // and `list_highlights` must disclaim it. An edit that quietly makes
        // `list_highlights` sound like a starting point is exactly the
        // regression this catches.
        let tools = tools();
        let by_name = |n: &str| -> String {
            tools
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == n)
                .unwrap_or_else(|| panic!("{n} is missing"))["description"]
                .as_str()
                .unwrap()
                .to_string()
        };

        let stack = by_name("get_stack");
        assert!(stack.starts_with("Start here."), "{stack:?}");
        assert!(
            stack.contains("in the order they asked"),
            "get_stack must promise the human's asking order: {stack:?}"
        );

        let list = by_name("list_highlights");
        assert!(
            list.contains("This is not how you find work"),
            "list_highlights must disclaim being the entry point: {list:?}"
        );
        assert!(
            list.contains("get_stack"),
            "list_highlights must name the tool it defers to: {list:?}"
        );

        assert!(
            INSTRUCTIONS.contains("Start with get_stack"),
            "the server instructions must name the entry point"
        );
    }

    #[test]
    fn nothing_in_the_schema_promises_to_write_a_file() {
        // Tenet 1. The tool surface is read plus annotation state; file
        // mutation stays with the agent's own Edit/Write. A future tool
        // described as writing would be a tenet change, not a feature.
        for tool in tools().as_array().unwrap() {
            let desc = tool["description"].as_str().unwrap().to_lowercase();
            let name = tool["name"].as_str().unwrap();
            for banned in ["edits the file", "writes the file", "modifies the file"] {
                assert!(!desc.contains(banned), "{name} claims to {banned}");
            }
        }
        assert!(INSTRUCTIONS.contains("dreamd never writes to the user's markdown"));
    }

    #[test]
    fn initialize_reports_the_protocol_version_and_the_instructions() {
        let init = initialize_result();
        assert_eq!(init["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(init["serverInfo"]["name"], "dreamd");
        assert_eq!(init["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        // `listChanged: false` is honest: the tool list is a compiled-in const
        // and cannot change while the process lives.
        assert_eq!(init["capabilities"]["tools"]["listChanged"], false);
        assert!(init["instructions"].as_str().unwrap().contains("get_stack"));
    }

    #[test]
    fn tools_list_wraps_the_array_under_a_tools_key() {
        let result = tools_list_result();
        assert_eq!(result["tools"], tools());
        assert_eq!(tool_names().len(), 6);
    }
}
