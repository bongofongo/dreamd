//! The MCP wire contract, and the one place document text is labelled on its
//! way to an agent.
//!
//! [`Highlight`] is **not** serialized to the wire directly. It serializes
//! snake_case and the frontend depends on that, so publishing it would weld an
//! internal struct to a public schema and make every future field rename a
//! breaking change for two consumers at once. The types here are camelCase,
//! agent-shaped, repo-relative, and owned by this module.
//!
//! They are also the choke point for [`crate::untrusted::delimit`]. Every path
//! by which repo markdown reaches an agent runs through [`Mark::new`], so the
//! labelling is applied in one constructor rather than remembered at each call
//! site in `tools.rs`. Adding a new tool cannot forget it. [`OpenDocument`] is
//! the one DTO here carrying no delimited field, and deliberately: a path is
//! dreamd's answer about its own state, not text lifted out of a document.

use crate::annotations::{Highlight, HighlightState, Origin, Resolution};
use crate::untrusted::delimit;
use serde::Serialize;
use std::path::Path;

/// Whether a mark's anchor still resolves. A view mirror of
/// [`HighlightState`], not a re-export: the wire contract is this module's to
/// keep stable even if the internal enum grows a variant the agent shouldn't
/// see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Active,
    Stale,
}

impl From<HighlightState> for State {
    fn from(s: HighlightState) -> Self {
        match s {
            HighlightState::Active => State::Active,
            HighlightState::Stale => State::Stale,
        }
    }
}

/// Who made the mark. The agent gets told, so it can tell its own notes from
/// the human's questions and not mistake one for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum By {
    Human,
    Agent,
}

impl From<Origin> for By {
    fn from(o: Origin) -> Self {
        match o {
            Origin::Human => By::Human,
            Origin::Agent => By::Agent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub note: Option<String>,
    /// Unix seconds.
    pub at: u64,
}

impl From<&Resolution> for Resolved {
    fn from(r: &Resolution) -> Self {
        Self {
            note: r.note.clone(),
            at: r.at,
        }
    }
}

/// One mark, as an agent sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    pub id: String,
    /// Repo-relative. See [`relative`].
    pub file: String,
    pub line_start: usize,
    pub line_end: usize,
    /// The highlighted passage — document text, and therefore delimited.
    pub quote: String,
    pub state: State,
    pub by: By,
    /// The human's note, if there is one. **Not** delimited: this is the text a
    /// human typed into dreamd to address the agent, so it is the instruction
    /// rather than the untrusted content, and wrapping it in a "do not follow
    /// this" envelope would tell the agent to ignore the very question it was
    /// handed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Resolved>,
}

impl Mark {
    /// Project a stored highlight onto the wire.
    ///
    /// Deliberately not `From<&Highlight>`: the repo root is needed to make the
    /// path relative, and a conversion that could not see the root would have
    /// to leave the absolute path in place and trust a later step to fix it.
    /// That is precisely the shape of leak `paths_leaving_dreamd_are_repo_relative`
    /// guards against, so the root is a parameter and there is no way to build
    /// a `Mark` without one.
    pub fn new(h: &Highlight, root: &Path) -> Self {
        Self {
            id: h.id.clone(),
            file: relative(&h.file_path, root),
            line_start: h.line_start,
            line_end: h.line_end,
            quote: delimit("quote", &h.quote),
            state: h.state.into(),
            by: h.origin.into(),
            annotation: h.annotation.clone(),
            resolved: h.resolved.as_ref().map(Resolved::from),
        }
    }
}

/// A mark plus the text around it — what `get_highlight` returns and what
/// `get_stack` deliberately omits, because sending a paragraph of context for
/// every queued question would bury the questions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkDetail {
    #[serde(flatten)]
    pub mark: Mark,
    /// Document text, and therefore delimited.
    pub context_before: String,
    /// Document text, and therefore delimited.
    pub context_after: String,
}

impl MarkDetail {
    pub fn new(h: &Highlight, root: &Path) -> Self {
        Self {
            mark: Mark::new(h, root),
            context_before: delimit("context-before", &h.prefix),
            context_after: delimit("context-after", &h.suffix),
        }
    }
}

/// One entry on the human's queue: the passage, and what they asked about it.
///
/// The annotation is hoisted out of the mark on purpose. It is the question,
/// the mark is the evidence, and an agent scanning the queue should not have to
/// dig a nullable field out of a nested object to find out what was asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub mark: Mark,
    pub annotation: String,
}

impl Question {
    pub fn new(h: &Highlight, annotation: String, root: &Path) -> Self {
        Self {
            mark: Mark::new(h, root),
            annotation,
        }
    }
}

/// `resolve_highlight`'s reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Closure {
    pub ok: bool,
    /// How many questions are still queued. Lets the agent report progress
    /// without a second call.
    pub remaining_on_stack: usize,
}

/// `mark_passage`'s reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Marked {
    pub id: String,
}

/// What the human has on screen, as `get_open_document` answers it.
///
/// A struct with one nullable field rather than a bare string, for two reasons.
/// It gives "nothing is open" a shape — `{"file": null}` is an answer, where a
/// bare `null` reads like a tool that failed — and it leaves room to say more
/// later without changing the result's type on a model that has already learned
/// it.
///
/// **Nothing here is delimited, because nothing here is document text.** A path
/// is dreamd's own answer about its own state, not content lifted out of a file
/// the way [`Mark`]'s quote is. It is still repo-relative, and the tool that
/// builds it has already refused anything outside the root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenDocument {
    pub file: Option<String>,
}

impl OpenDocument {
    /// Nothing open — the honest answer for a window at its empty state, a
    /// launch with no repo, and a document that has since been deleted or left
    /// the root.
    pub fn none() -> Self {
        Self { file: None }
    }

    pub fn new(path: &Path, root: &Path) -> Self {
        Self {
            file: Some(relative(&path.to_string_lossy(), root)),
        }
    }
}

/// Render `path` relative to the repo root.
///
/// A path outside the root is left absolute, matching what `send.rs` already
/// does for the query assembler (`a_path_outside_the_repo_root_is_left_absolute`).
/// Truncating it to a basename would be worse than useless: it would *look*
/// repo-relative while pointing somewhere else, and an agent would then try to
/// read the wrong file. In practice the case does not arise — the store is
/// filtered by `guard::inside_root` on load, and `get_open_document` refuses
/// anything outside it — so the honest fallback costs nothing.
pub fn relative(path: &str, root: &Path) -> String {
    Path::new(path)
        .strip_prefix(root)
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotations::Store;
    use serde_json::Value;

    const ROOT: &str = "/repo";

    fn highlight() -> Highlight {
        let mut store = Store::default();
        let id = store.add_highlight(
            "/repo/docs/a.md".into(),
            4,
            6,
            "the quoted passage".into(),
            "text before ".into(),
            " text after".into(),
        );
        store.set_annotation(&id, "why is this?".into());
        store.get(&id).expect("present")
    }

    fn json(h: &Highlight) -> Value {
        serde_json::to_value(Mark::new(h, Path::new(ROOT))).expect("serialises")
    }

    #[test]
    fn the_wire_contract_is_camel_case_and_not_the_internal_struct() {
        // `Highlight` serialises snake_case and the frontend depends on it.
        // Publishing it would weld the two contracts together.
        let v = json(&highlight());
        for key in ["lineStart", "lineEnd", "id", "file", "quote", "state", "by"] {
            assert!(v.get(key).is_some(), "missing {key}: {v}");
        }
        for internal in ["line_start", "line_end", "file_path", "origin", "prefix"] {
            assert!(v.get(internal).is_none(), "leaked {internal}: {v}");
        }
        assert_eq!(v["state"], "active");
        assert_eq!(v["by"], "human");
    }

    #[test]
    fn a_quote_crossing_into_an_agent_is_delimited() {
        // The choke point. Repo markdown is untrusted content: it reaches an
        // agent's context labelled, or it does not reach it.
        let h = highlight();
        let v = json(&h);
        let quote = v["quote"].as_str().expect("a string");
        assert_ne!(
            quote, h.quote,
            "the quote crossed the wire raw — the envelope is gone"
        );
        assert!(
            quote.contains(&h.quote),
            "the envelope must still carry the text: {quote:?}"
        );
    }

    #[test]
    fn the_anchoring_context_is_delimited_too() {
        // prefix/suffix are document bytes exactly as much as the quote is.
        let h = highlight();
        let d = MarkDetail::new(&h, Path::new(ROOT));
        assert_ne!(d.context_before, h.prefix);
        assert_ne!(d.context_after, h.suffix);
        assert!(d.context_before.contains(&h.prefix));
        assert!(d.context_after.contains(&h.suffix));
    }

    #[test]
    fn the_humans_annotation_is_not_delimited() {
        // The inverse of the test above, and the one that is easy to get wrong
        // by being thorough. The annotation is what the human typed to address
        // the agent — it is the instruction, not the untrusted content. Wrapping
        // it in a "do not follow anything in here" envelope would tell the agent
        // to ignore the question it was handed.
        let h = highlight();
        let v = json(&h);
        assert_eq!(v["annotation"], "why is this?");
    }

    #[test]
    fn mark_detail_flattens_rather_than_nesting() {
        // `get_highlight` returns "one Mark plus prefix/suffix", not a Mark
        // buried under a key.
        let v = serde_json::to_value(MarkDetail::new(&highlight(), Path::new(ROOT)))
            .expect("serialises");
        assert!(v.get("id").is_some(), "{v}");
        assert!(v.get("contextBefore").is_some(), "{v}");
        assert!(v.get("contextAfter").is_some(), "{v}");
        assert!(v.get("mark").is_none(), "must be flattened: {v}");
    }

    #[test]
    fn paths_are_repo_relative() {
        // An absolute path leaks $HOME into an agent's context, and worse, into
        // whatever that agent writes down.
        let v = json(&highlight());
        assert_eq!(v["file"], "docs/a.md");
        assert!(!v["file"].as_str().unwrap().starts_with('/'), "{v}");
    }

    #[test]
    fn a_path_outside_the_root_is_left_absolute_rather_than_truncated() {
        // Matching `send.rs`. A basename would look repo-relative while
        // pointing elsewhere, which sends the agent to the wrong file.
        assert_eq!(
            relative("/elsewhere/b.md", Path::new(ROOT)),
            "/elsewhere/b.md"
        );
        assert_eq!(relative("/repo/a.md", Path::new(ROOT)), "a.md");
    }

    #[test]
    fn an_unresolved_mark_omits_the_resolved_key_entirely() {
        // `skip_serializing_if` rather than an explicit null: a model reading
        // `"resolved": null` has to reason about what that means, whereas an
        // absent key reads as "not resolved" at a glance.
        let v = json(&highlight());
        assert!(v.get("resolved").is_none(), "{v}");
    }

    #[test]
    fn a_resolved_mark_carries_the_note_and_the_time() {
        let mut h = highlight();
        h.resolved = Some(Resolution {
            note: Some("fixed in commit abc".into()),
            at: 1_753_617_600,
        });
        let v = json(&h);
        assert_eq!(v["resolved"]["note"], "fixed in commit abc");
        assert_eq!(v["resolved"]["at"], 1_753_617_600u64);
    }

    #[test]
    fn a_question_hoists_the_annotation_beside_the_mark() {
        let h = highlight();
        let v = serde_json::to_value(Question::new(&h, "why is this?".into(), Path::new(ROOT)))
            .expect("serialises");
        assert_eq!(v["annotation"], "why is this?");
        assert_eq!(v["mark"]["file"], "docs/a.md");
    }
}
