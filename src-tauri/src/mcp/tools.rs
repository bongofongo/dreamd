//! Tool dispatch: the whole agent-facing surface, and nothing else.
//!
//! [`dispatch`] takes a `&mut Store`, the repo root, and a call. **No
//! `AppHandle`, no `AppState`, no Tauri type at all.** That is not tidiness —
//! it is what lets every tool be driven from a unit test with the same `Store`
//! fixtures `annotations.rs` uses, with no window, no socket and no event loop.
//! The transport that calls in here lives elsewhere (step 3c).
//!
//! ## The shape of the surface
//!
//! Four read tools, two write tools, and none of them writes a file byte.
//! Tenet 1 holds: dreamd exposes reading and annotation *state*; changing a
//! document stays with the agent's own `Edit`/`Write`. `get_open_document` is
//! the one that reports the *window* rather than the store, and it still only
//! reports: dreamd never opens or closes a document on an agent's behalf.
//!
//! What is deliberately absent matters as much as what is here:
//!
//! - **Nothing reorders the stack.** Its order is the order the human asked in,
//!   and `annotations.rs`'s `re_annotating_replaces_the_text_without_reordering_the_stack`
//!   pins that internally. An agent must not be able to break it from outside.
//! - **Nothing deletes a highlight.** Destroying the human's marks is not the
//!   agent's call; [`resolve_highlight`](dispatch) covers the legitimate case
//!   and keeps the evidence.
//! - **Nothing returns a document.** `read_marked_document` was considered and
//!   cut: it only serves a file-first flow, and queue-first never needs a
//!   document laid out, because `get_stack` already hands over each question
//!   with its quote attached. The argument against it is redundancy, not
//!   danger — the agent can read the same bytes with its own tool either way.

use crate::annotations::{Origin, Store};
use crate::guard;
use crate::mcp::view::{Closure, Mark, MarkDetail, Marked, OpenDocument, Question, State};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Every tool name dispatch answers to, in schema order.
///
/// One list, cross-checked against `schema::TOOLS` in both directions by
/// `every_tool_in_the_schema_has_a_dispatch_arm` and its converse. Drift
/// between the published schema and the implemented `match` is the failure this
/// module is most likely to suffer and least likely to notice.
pub const NAMES: &[&str] = &[
    "get_stack",
    "get_open_document",
    "get_highlight",
    "resolve_highlight",
    "list_highlights",
    "mark_passage",
];

/// One `tools/call`.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

impl ToolCall {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            name: name.into(),
            arguments,
        }
    }
}

/// Why a call could not be answered.
///
/// The variants exist so the security-shaped tests can assert on *which*
/// refusal happened rather than grepping a message. [`Refused`](ToolError::Refused)
/// in particular is only ever produced by the containment guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// No such tool. Its own variant so the schema/dispatch drift test can tell
    /// "not implemented" from "implemented and unhappy".
    UnknownTool(String),
    /// The arguments were missing, of the wrong type, or unusable.
    InvalidArguments(String),
    /// The id named nothing.
    NotFound(String),
    /// A tenet said no.
    Refused(String),
    /// Something failed that was nobody's fault, e.g. a file read.
    Failed(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::UnknownTool(n) => {
                write!(f, "no such tool: {n}. dreamd offers {}", NAMES.join(", "))
            }
            ToolError::InvalidArguments(m)
            | ToolError::NotFound(m)
            | ToolError::Refused(m)
            | ToolError::Failed(m) => write!(f, "{m}"),
        }
    }
}

pub type ToolResult = Result<Value, ToolError>;

/// Run one tool.
///
/// The single entry point, and the seam the whole module is arranged around.
///
/// `open_doc` is the absolute path of the document the human has on screen, or
/// `None`. It arrives already *resolved* — a plain `Option<&Path>` rather than
/// the [`super::OpenDoc`] closure that produced it — so this module stays as
/// free of the window as it was: the caller reads it once, under whatever lock
/// it lives behind, and hands in a value.
pub fn dispatch(
    store: &mut Store,
    root: &Path,
    open_doc: Option<&Path>,
    call: &ToolCall,
) -> ToolResult {
    let args = &call.arguments;
    match call.name.as_str() {
        "get_stack" => get_stack(store, root),
        "get_open_document" => get_open_document(open_doc, root),
        "get_highlight" => get_highlight(store, root, args),
        "resolve_highlight" => resolve_highlight(store, args),
        "list_highlights" => list_highlights(store, root, args),
        "mark_passage" => mark_passage(store, root, args),
        other => Err(ToolError::UnknownTool(other.to_string())),
    }
}

/// [`dispatch`], wrapped in MCP's `tools/call` result envelope.
///
/// A failed tool is **not** a JSON-RPC error: the spec asks for a normal result
/// with `isError: true`, precisely so the model sees the message and can
/// correct itself instead of the client swallowing a protocol fault. That is
/// what makes "the quote is not in that file" a usable instruction rather than
/// a dead end.
pub fn call(store: &mut Store, root: &Path, open_doc: Option<&Path>, call: &ToolCall) -> Value {
    match dispatch(store, root, open_doc, call) {
        Ok(value) => {
            let text = serde_json::to_string_pretty(&value)
                .unwrap_or_else(|e| format!("could not render the result: {e}"));
            let mut out = json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false,
            });
            // `structuredContent` must be an object, and `get_stack` answers
            // with an array. Rather than wrapping the array in a synthetic key
            // and changing the documented shape, the structured copy is simply
            // omitted where it does not apply; the text copy is always there.
            if value.is_object() {
                out["structuredContent"] = value;
            }
            out
        }
        Err(e) => json!({
            "content": [{ "type": "text", "text": e.to_string() }],
            "isError": true,
        }),
    }
}

// ---- the tools ----------------------------------------------------------

/// The human's open questions, in order.
///
/// The entry point. Order comes from the stack, which is append-on-first-
/// annotation, so it is the order the human asked in — not id order, not file
/// order, and not anything an agent can influence. Highlights that were never
/// annotated are skipped by `stack_pairs` itself: an unannotated mark is
/// something the human underlined while reading, not a question, and must not
/// arrive as one.
fn get_stack(store: &Store, root: &Path) -> ToolResult {
    let questions: Vec<Question> = store
        .stack_pairs()
        .into_iter()
        .map(|p| Question::new(&p.highlight, p.annotation, root))
        .collect();
    to_value(&questions)
}

/// The document the human has on screen.
///
/// **Validated here rather than trusted**, even though the path came from
/// dreamd's own window rather than from the agent. It is the one value in this
/// module that arrives from outside the store, it is minted by whatever the
/// frontend last asked to render, and it outlives the state that produced it —
/// a File ▸ Open swaps the root under it, and a document can be deleted while
/// the window still shows it. Re-checking containment and existence on every
/// call is what makes each of those answer "nothing open" instead of naming a
/// file that is gone or that belongs to a repo this server is no longer
/// serving.
///
/// A refusal would be the wrong shape: the agent asked a question about the
/// human's window, and "they have nothing open" is a true answer to it, where
/// an error would read as a tool that failed. So every failure lands on
/// [`OpenDocument::none`], and none of them says which failure it was — the
/// same reason `resolve_in_root` gives one message for two causes.
fn get_open_document(open_doc: Option<&Path>, root: &Path) -> ToolResult {
    let answer = open_doc
        .and_then(|p| p.canonicalize().ok())
        .filter(|p| {
            let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            guard::inside_root(&root, p)
        })
        .map(|p| {
            let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            OpenDocument::new(&p, &root)
        })
        .unwrap_or_else(OpenDocument::none);
    to_value(&answer)
}

/// One mark, with the text either side of it.
fn get_highlight(store: &Store, root: &Path, args: &Value) -> ToolResult {
    let id = string_arg(args, "id")?;
    let h = store
        .get(&id)
        .ok_or_else(|| ToolError::NotFound(format!("no mark with id {id}")))?;
    to_value(&MarkDetail::new(&h, root))
}

/// Close a question. The loop-closer, and the reason this beats an outbox.
///
/// The highlight survives: the evidence and the agent's note stay anchored to
/// the passage, while the stack entry goes so the badge decrements and the GUI
/// repaints. "I flagged four things, address them" → "addressed" becomes a
/// cycle rather than a one-way send.
fn resolve_highlight(store: &mut Store, args: &Value) -> ToolResult {
    let id = string_arg(args, "id")?;
    let note = optional_string_arg(args, "note")?;

    // `Store::resolve` records the answer and clears the stack entry in one
    // pass, without disturbing the reanchor gate. Doing it out here through
    // `into_parts`/`from_parts` cost a wasted `SourceIndex` rebuild per mark.
    if !store.resolve(&id, note) {
        return Err(ToolError::NotFound(format!("no mark with id {id}")));
    }

    to_value(&Closure {
        ok: true,
        remaining_on_stack: store.stack_pairs().len(),
    })
}

/// Marks in a file the agent is already working in. The *secondary* entry.
fn list_highlights(store: &Store, root: &Path, args: &Value) -> ToolResult {
    let file = optional_string_arg(args, "file")?;
    let state = optional_string_arg(args, "state")?
        .map(|s| match s.as_str() {
            "active" => Ok(State::Active),
            "stale" => Ok(State::Stale),
            other => Err(ToolError::InvalidArguments(format!(
                "state must be \"active\" or \"stale\", got {other:?}"
            ))),
        })
        .transpose()?;
    let resolved = optional_bool_arg(args, "resolved")?;

    // Filtering only ever narrows a set that is already inside the root, so
    // this resolution is lenient where `mark_passage`'s is not: a filter naming
    // a file that has since been deleted should return that file's marks, not
    // an error, and it cannot reach anything the store does not already hold.
    let wanted = file.map(|f| {
        let joined = root.join(&f);
        joined
            .canonicalize()
            .unwrap_or(joined)
            .to_string_lossy()
            .into_owned()
    });

    // Spelled as `match` rather than `Option::is_none_or`, which is Rust 1.82
    // and this crate's `rust-version` is 1.77.
    let (highlights, _) = store.parts();
    let marks: Vec<Mark> = highlights
        .iter()
        .filter(|h| match &wanted {
            Some(w) => &h.file_path == w,
            None => true,
        })
        .filter(|h| match state {
            Some(s) => State::from(h.state) == s,
            None => true,
        })
        .filter(|h| match resolved {
            Some(r) => h.resolved.is_some() == r,
            None => true,
        })
        .map(|h| Mark::new(h, root))
        .collect();
    to_value(&marks)
}

/// The agent's own note on a passage.
///
/// `file` resolves against the repo root and **must** pass
/// `guard::inside_root` before anything happens — canonicalisation first, so
/// `../` is resolved away before containment is tested rather than after.
fn mark_passage(store: &mut Store, root: &Path, args: &Value) -> ToolResult {
    let file = string_arg(args, "file")?;
    let quote = string_arg(args, "quote")?;
    let note = string_arg(args, "note")?;

    let path = resolve_in_root(root, &file)?;
    let path_str = path.to_string_lossy().into_owned();
    let source = crate::read_source(&path_str).map_err(ToolError::Failed)?;

    // Refuse rather than anchoring at line 0. `add_anchored`'s fallback exists
    // for the frontend, whose selections come back from the DOM with whitespace
    // already collapsed; an agent supplies source text and can be told to try
    // again, which is far better than a mark silently landing at the top of the
    // file.
    if crate::markdown::locate(&source, "", &quote, "").is_none() {
        return Err(ToolError::InvalidArguments(format!(
            "that quote does not appear in {file}; copy it verbatim from the file"
        )));
    }

    let id = store.add_anchored(
        path_str,
        &source,
        quote,
        String::new(),
        String::new(),
        Origin::Agent,
    );
    // `set_annotation` is also what enqueues, and the stack is the *human's*
    // queue of questions for the agent. An agent's own note belongs on the
    // document, not on the queue it is working through — leaving it there would
    // inflate the human's badge and hand the agent its own notes back as
    // questions on the next `get_stack`. The frozen `Store` API offers no way
    // to annotate without enqueueing, so it is undone immediately.
    store.set_annotation(&id, note);
    store.remove_from_stack(&id);

    to_value(&Marked { id })
}

// ---- plumbing -----------------------------------------------------------

/// Resolve `file` against the repo root, refusing anything that lands outside.
///
/// Canonicalisation precedes containment, which is the entire point: without
/// it, `docs/../../../etc/passwd` starts with the root textually and would pass
/// a naive check. `guard::inside_root` compares whole components, so a sibling
/// sharing a prefix (`/w/notes-private` under a root of `/w/notes`) is outside
/// too.
fn resolve_in_root(root: &Path, file: &str) -> Result<PathBuf, ToolError> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let target = root.join(file).canonicalize().map_err(|_| {
        // Deliberately vague and deliberately identical for "does not exist"
        // and "cannot be reached": distinguishing them turns this into an
        // oracle for what exists outside the repo.
        ToolError::Refused(format!("no such file in this repo: {file}"))
    })?;
    if !guard::inside_root(&root, &target) {
        return Err(ToolError::Refused(format!(
            "refusing a path outside the repo root: {file}"
        )));
    }
    Ok(target)
}

fn to_value<T: serde::Serialize>(value: &T) -> ToolResult {
    serde_json::to_value(value)
        .map_err(|e| ToolError::Failed(format!("could not render the result: {e}")))
}

fn string_arg(args: &Value, key: &str) -> Result<String, ToolError> {
    match args.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(ToolError::InvalidArguments(format!(
            "{key} must be a string, got {other}"
        ))),
        None => Err(ToolError::InvalidArguments(format!("{key} is required"))),
    }
}

fn optional_string_arg(args: &Value, key: &str) -> Result<Option<String>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(ToolError::InvalidArguments(format!(
            "{key} must be a string, got {other}"
        ))),
    }
}

fn optional_bool_arg(args: &Value, key: &str) -> Result<Option<bool>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(other) => Err(ToolError::InvalidArguments(format!(
            "{key} must be true or false, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::schema;
    use std::fs;

    /// A scratch repo on disk. Only `mark_passage` needs one — everything else
    /// runs against an in-memory `Store` with no filesystem at all.
    struct Repo {
        root: PathBuf,
    }

    impl Repo {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "dreamd-mcp-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("docs")).expect("scratch repo");
            fs::write(root.join("docs/a.md"), FILE).expect("fixture");
            Self {
                root: root.canonicalize().unwrap_or(root),
            }
        }

        fn path(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    const FILE: &str = "# Title\n\nthe first paragraph\n\nthe second paragraph\n";

    fn store_with(quotes: &[(&str, Option<&str>)]) -> (Store, Vec<String>) {
        let mut store = Store::default();
        let ids = quotes
            .iter()
            .map(|(quote, note)| {
                let id = store.add_highlight(
                    "/repo/docs/a.md".into(),
                    1,
                    1,
                    (*quote).to_string(),
                    String::new(),
                    String::new(),
                );
                if let Some(n) = note {
                    store.set_annotation(&id, (*n).to_string());
                }
                id
            })
            .collect();
        (store, ids)
    }

    fn run(store: &mut Store, name: &str, args: Value) -> ToolResult {
        dispatch(store, Path::new("/repo"), None, &ToolCall::new(name, args))
    }

    /// `run`, for the one tool whose answer is about the window rather than the
    /// store.
    fn run_doc(root: &Path, open: Option<&Path>) -> ToolResult {
        let mut store = Store::default();
        dispatch(
            &mut store,
            root,
            open,
            &ToolCall::new("get_open_document", json!({})),
        )
    }

    // ---- the drift guard --------------------------------------------------

    #[test]
    fn every_tool_in_the_schema_has_a_dispatch_arm() {
        // The most valuable test in the module. A tool published in
        // `tools/list` that dispatch does not answer to is a tool Claude Code
        // will happily call and always fail — and nothing else would catch it,
        // because the schema is a hand-written const.
        let mut store = Store::default();
        for name in schema::tool_names() {
            let err = run(&mut store, &name, json!({})).err();
            assert!(
                !matches!(err, Some(ToolError::UnknownTool(_))),
                "{name} is published but has no dispatch arm"
            );
        }
    }

    #[test]
    fn every_dispatch_arm_is_in_the_schema() {
        // The converse: a tool that works but is never advertised is dead code
        // an agent can never reach.
        let published = schema::tool_names();
        for name in NAMES {
            assert!(
                published.iter().any(|p| p == name),
                "{name} dispatches but is not published in the schema"
            );
        }
        assert_eq!(published.len(), NAMES.len(), "the two lists differ in size");
    }

    #[test]
    fn an_unknown_tool_name_is_an_error_not_a_panic() {
        let mut store = Store::default();
        let err = run(&mut store, "read_marked_document", json!({})).unwrap_err();
        assert_eq!(
            err,
            ToolError::UnknownTool("read_marked_document".into()),
            "the cut tool must stay cut"
        );
        // The message names what does exist, so a model can correct itself.
        assert!(err.to_string().contains("get_stack"), "{err}");

        // ...and the `tools/call` envelope turns it into a visible result
        // rather than a protocol fault.
        let v = call(
            &mut store,
            Path::new("/repo"),
            None,
            &ToolCall::new("nonsense", json!({})),
        );
        assert_eq!(v["isError"], true);
        assert!(v["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("no such tool"));
    }

    // ---- get_stack --------------------------------------------------------

    #[test]
    fn get_stack_returns_questions_in_the_order_the_human_asked_them() {
        // The queue-first loop's core promise, asserted across the wire
        // boundary. `annotations.rs` pins no-reordering internally; this pins
        // that the projection preserves it.
        let (mut store, ids) = store_with(&[("alpha", None), ("beta", None), ("gamma", None)]);
        store.set_annotation(&ids[2], "asked third about gamma".into());
        store.set_annotation(&ids[0], "asked first about alpha".into());
        store.set_annotation(&ids[1], "asked second about beta".into());
        // Re-annotating must not move an entry to the back.
        store.set_annotation(&ids[2], "revised, still first in line".into());

        let v = run(&mut store, "get_stack", json!({})).expect("ok");
        let arr = v.as_array().expect("an array");
        assert_eq!(arr.len(), 3);
        assert_eq!(
            arr.iter()
                .map(|q| q["mark"]["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![ids[2].as_str(), ids[0].as_str(), ids[1].as_str()],
        );
        assert_eq!(arr[0]["annotation"], "revised, still first in line");
    }

    #[test]
    fn get_stack_skips_a_highlight_that_was_never_annotated() {
        // A mark the human underlined while reading is not a question, and must
        // not reach the agent as one.
        let (mut store, _) = store_with(&[("alpha", None), ("beta", Some("why?"))]);
        let v = run(&mut store, "get_stack", json!({})).expect("ok");
        let arr = v.as_array().expect("an array");
        assert_eq!(arr.len(), 1, "{v}");
        assert_eq!(arr[0]["annotation"], "why?");
    }

    #[test]
    fn get_stack_takes_no_arguments_and_tolerates_being_given_some() {
        let (mut store, _) = store_with(&[("alpha", Some("why?"))]);
        assert!(run(&mut store, "get_stack", Value::Null).is_ok());
        assert!(run(&mut store, "get_stack", json!({"junk": 1})).is_ok());
    }

    #[test]
    fn paths_leaving_dreamd_are_repo_relative() {
        // The invariant `send.rs` already pins for the query assembler. An
        // absolute path leaks $HOME into agent context — and into whatever the
        // agent writes down afterwards.
        let (mut store, _) = store_with(&[("alpha", Some("why?"))]);
        let v = run(&mut store, "get_stack", json!({})).expect("ok");
        assert_eq!(v[0]["mark"]["file"], "docs/a.md");
        assert!(
            !v.to_string().contains("/repo/docs/a.md"),
            "absolute path leaked: {v}"
        );
    }

    // ---- get_open_document ------------------------------------------------

    #[test]
    fn the_open_document_comes_back_repo_relative() {
        let repo = Repo::new("opendoc-rel");
        let v = run_doc(repo.path(), Some(&repo.path().join("docs/a.md"))).unwrap();
        assert_eq!(v["file"], "docs/a.md");
        // The same rule every other tool obeys: nothing leaving dreamd names a
        // path on this machine.
        assert!(
            !v.to_string()
                .contains(repo.path().to_string_lossy().as_ref()),
            "absolute path leaked: {v}"
        );
    }

    #[test]
    fn nothing_open_is_an_answer_rather_than_an_error() {
        let repo = Repo::new("opendoc-none");
        // `Ok`, not `Err`. The agent asked what the human is reading and
        // "nothing" is a true answer; a refusal would read as a broken tool and
        // invite a retry that cannot succeed.
        let v = run_doc(repo.path(), None).expect("an answer, not a refusal");
        assert_eq!(v["file"], Value::Null);
        // The key is present and null rather than absent, so a model that
        // learned the shape does not have to treat a missing field as a
        // protocol change.
        assert!(v.as_object().expect("an object").contains_key("file"));
    }

    #[test]
    fn a_document_that_no_longer_exists_reads_as_nothing_open() {
        let repo = Repo::new("opendoc-gone");
        // The slot is never cleared — a file deleted under the window, and a
        // root swapped by File ▸ Open, both leave a path pointing at nothing.
        // Re-checking on read is what makes those answer honestly instead of
        // naming a file that is gone.
        let v = run_doc(repo.path(), Some(&repo.path().join("docs/gone.md"))).unwrap();
        assert_eq!(v["file"], Value::Null);
    }

    #[test]
    fn a_document_outside_the_root_is_never_named() {
        let repo = Repo::new("opendoc-outside");
        let outside = repo
            .path()
            .parent()
            .unwrap()
            .join("dreamd-opendoc-secret.md");
        fs::write(&outside, "# secret\n").unwrap();
        let v = run_doc(repo.path(), Some(&outside)).unwrap();
        assert_eq!(v["file"], Value::Null, "named a file outside the root");
        assert!(!v.to_string().contains("secret"), "{v}");
        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn a_sibling_sharing_the_roots_prefix_is_outside_it() {
        // `guard::inside_root` compares whole components precisely so that
        // `/w/notes-private` is not inside `/w/notes`. This is that rule
        // restated where a *textual* `starts_with` would have passed — the
        // check `resolve_in_root` gets right for the tools that take a `file`
        // argument, asserted for the one that does not.
        let repo = Repo::new("opendoc-prefix");
        let sibling = repo.path().with_file_name(format!(
            "{}-private",
            repo.path().file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("secret.md"), "# secret\n").unwrap();
        let v = run_doc(repo.path(), Some(&sibling.join("secret.md"))).unwrap();
        assert_eq!(v["file"], Value::Null, "a prefix match is not containment");
        let _ = fs::remove_dir_all(&sibling);
    }

    #[test]
    fn traversal_out_of_the_root_and_back_is_resolved_before_it_is_judged() {
        // Canonicalisation precedes containment, the same order
        // `resolve_in_root` uses: a path that leaves the root and returns is
        // inside it, and one that only leaves is not — and neither answer may
        // depend on how the path was spelled.
        let repo = Repo::new("opendoc-traversal");
        let back = repo.path().join("docs/../docs/a.md");
        assert_eq!(
            run_doc(repo.path(), Some(&back)).unwrap()["file"],
            "docs/a.md"
        );
        let outside = repo.path().parent().unwrap().join("dreamd-opendoc-up.md");
        fs::write(&outside, "# out\n").unwrap();
        let up = repo.path().join("docs/../../dreamd-opendoc-up.md");
        assert_eq!(
            run_doc(repo.path(), Some(&up)).unwrap()["file"],
            Value::Null
        );
        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn asking_what_is_open_changes_nothing() {
        // A read tool, and the only one that could plausibly be mistaken for a
        // navigation command. dreamd never opens, closes or writes a document
        // on an agent's behalf (tenet 1), and the tool takes no arguments with
        // which to ask.
        let published = schema::tools();
        let tool = published
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "get_open_document")
            .expect("published");
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(
            tool["inputSchema"]["properties"]
                .as_object()
                .map(|o| o.len()),
            Some(0),
            "it must not grow a way to ask for a *different* document"
        );
    }

    // ---- get_highlight ----------------------------------------------------

    #[test]
    fn get_highlight_carries_the_anchoring_context() {
        let mut store = Store::default();
        let id = store.add_highlight(
            "/repo/docs/a.md".into(),
            3,
            3,
            "the first paragraph".into(),
            "# Title\n\n".into(),
            "\n\nthe second".into(),
        );
        let v = run(&mut store, "get_highlight", json!({"id": id})).expect("ok");
        assert_eq!(v["id"], id);
        assert_eq!(v["lineStart"], 3);
        assert!(v["contextBefore"].as_str().unwrap().contains("# Title"));
        assert!(v["contextAfter"].as_str().unwrap().contains("the second"));
        // Document text arrives labelled, never raw.
        assert_ne!(v["quote"], "the first paragraph");
        assert!(v["quote"].as_str().unwrap().contains("the first paragraph"));
    }

    #[test]
    fn get_highlight_of_an_unknown_id_is_an_error() {
        let mut store = Store::default();
        let err = run(
            &mut store,
            "get_highlight",
            json!({"id": "h0000000000000000"}),
        )
        .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)), "{err:?}");
        // A missing argument is a different failure from a missing mark.
        let err = run(&mut store, "get_highlight", json!({})).unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
        let err = run(&mut store, "get_highlight", json!({"id": 7})).unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    }

    // ---- resolve_highlight ------------------------------------------------

    #[test]
    fn resolve_highlight_leaves_the_highlight_and_clears_the_stack_entry() {
        let (mut store, ids) = store_with(&[("alpha", Some("why?")), ("beta", Some("and?"))]);
        let v = run(
            &mut store,
            "resolve_highlight",
            json!({"id": ids[0], "note": "because X"}),
        )
        .expect("ok");
        assert_eq!(v["ok"], true);
        assert_eq!(v["remainingOnStack"], 1);

        let h = store
            .get(&ids[0])
            .expect("the evidence survives resolution");
        assert_eq!(h.quote, "alpha");
        let resolved = h.resolved.expect("resolved is set");
        assert_eq!(resolved.note.as_deref(), Some("because X"));
        assert!(resolved.at > 0, "a resolution is timestamped");

        // Off the queue.
        let stack = run(&mut store, "get_stack", json!({})).expect("ok");
        assert_eq!(stack.as_array().unwrap().len(), 1);
        assert_eq!(stack[0]["mark"]["id"], ids[1]);
    }

    #[test]
    fn a_resolved_mark_leaves_the_stack_but_stays_in_list_highlights() {
        // Resolving closes the question without destroying the evidence — the
        // difference between this and an outbox.
        let (mut store, ids) = store_with(&[("alpha", Some("why?"))]);
        run(&mut store, "resolve_highlight", json!({"id": ids[0]})).expect("ok");

        assert!(run(&mut store, "get_stack", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());

        let all = run(&mut store, "list_highlights", json!({})).expect("ok");
        assert_eq!(all.as_array().unwrap().len(), 1, "{all}");
        assert_eq!(all[0]["id"], ids[0]);
        // Closed without a word: `ok` is still true, the note is just absent.
        assert_eq!(all[0]["resolved"]["note"], Value::Null);

        let open = run(&mut store, "list_highlights", json!({"resolved": false})).expect("ok");
        assert!(open.as_array().unwrap().is_empty(), "{open}");
        let closed = run(&mut store, "list_highlights", json!({"resolved": true})).expect("ok");
        assert_eq!(closed.as_array().unwrap().len(), 1);
    }

    #[test]
    fn resolving_an_unknown_id_is_an_error_and_changes_nothing() {
        let (mut store, _) = store_with(&[("alpha", Some("why?"))]);
        let err = run(
            &mut store,
            "resolve_highlight",
            json!({"id": "h0000000000000000"}),
        )
        .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)), "{err:?}");
        assert_eq!(store.stack_pairs().len(), 1, "the queue is untouched");
    }

    #[test]
    fn resolving_does_not_disturb_the_rest_of_the_store() {
        // Resolving touches exactly one highlight and one stack entry. This
        // pins that everything else — the other marks, their annotations, and
        // the queue order — is left alone.
        let (mut store, ids) = store_with(&[
            ("alpha", Some("first")),
            ("beta", Some("second")),
            ("gamma", None),
        ]);
        run(&mut store, "resolve_highlight", json!({"id": ids[1]})).expect("ok");

        let (highlights, stack) = store.parts();
        assert_eq!(highlights.len(), 3, "no highlight was lost");
        assert_eq!(stack, [ids[0].clone()], "order and membership survive");
        assert_eq!(
            store.get(&ids[0]).unwrap().annotation.as_deref(),
            Some("first")
        );
        assert!(store.get(&ids[2]).unwrap().annotation.is_none());
    }

    // ---- list_highlights --------------------------------------------------

    #[test]
    fn list_highlights_filters_by_file_and_state() {
        let mut store = Store::default();
        let a = store.add_highlight(
            "/repo/docs/a.md".into(),
            1,
            1,
            "alpha".into(),
            String::new(),
            String::new(),
        );
        store.add_highlight(
            "/repo/docs/b.md".into(),
            1,
            1,
            "beta".into(),
            String::new(),
            String::new(),
        );
        // Edit a.md's text out from under the mark: it goes stale, b.md's does
        // not.
        store.reanchor_file("/repo/docs/a.md", "nothing matches here\n");

        let all = run(&mut store, "list_highlights", json!({})).expect("ok");
        assert_eq!(all.as_array().unwrap().len(), 2);

        let one = run(&mut store, "list_highlights", json!({"file": "docs/a.md"})).expect("ok");
        assert_eq!(one.as_array().unwrap().len(), 1);
        assert_eq!(one[0]["id"], a);
        assert_eq!(one[0]["file"], "docs/a.md");

        let stale = run(&mut store, "list_highlights", json!({"state": "stale"})).expect("ok");
        assert_eq!(stale.as_array().unwrap().len(), 1);
        assert_eq!(stale[0]["id"], a);

        let active = run(&mut store, "list_highlights", json!({"state": "active"})).expect("ok");
        assert_eq!(active.as_array().unwrap().len(), 1);
        assert_eq!(active[0]["file"], "docs/b.md");

        // Filters compose, and a file nobody marked is empty rather than an
        // error.
        let none = run(
            &mut store,
            "list_highlights",
            json!({"file": "docs/a.md", "state": "active"}),
        )
        .expect("ok");
        assert!(none.as_array().unwrap().is_empty());
        let missing = run(
            &mut store,
            "list_highlights",
            json!({"file": "docs/gone.md"}),
        )
        .expect("a filter naming an unknown file is empty, not an error");
        assert!(missing.as_array().unwrap().is_empty());
    }

    #[test]
    fn list_highlights_rejects_a_state_it_does_not_know() {
        let (mut store, _) = store_with(&[("alpha", None)]);
        let err = run(&mut store, "list_highlights", json!({"state": "resolved"})).unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
        let err = run(&mut store, "list_highlights", json!({"resolved": "yes"})).unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    }

    // ---- mark_passage -----------------------------------------------------

    #[test]
    fn mark_passage_anchors_the_quote_and_records_an_agent_origin() {
        let repo = Repo::new("mark");
        let mut store = Store::default();
        let v = dispatch(
            &mut store,
            repo.path(),
            None,
            &ToolCall::new(
                "mark_passage",
                json!({
                    "file": "docs/a.md",
                    "quote": "the second paragraph",
                    "note": "this contradicts the first"
                }),
            ),
        )
        .expect("ok");

        let id = v["id"].as_str().expect("an id").to_string();
        let h = store.get(&id).expect("stored");
        assert_eq!(h.origin, Origin::Agent, "the frontend paints these apart");
        assert_eq!(h.line_start, 5, "anchored to the right line");
        assert_eq!(h.annotation.as_deref(), Some("this contradicts the first"));
    }

    #[test]
    fn mark_passage_does_not_put_the_agents_own_note_on_the_humans_queue() {
        // `set_annotation` is also what enqueues, so this is easy to get wrong
        // by accident. The stack is the human's questions *for* the agent;
        // seeding it with the agent's own notes would inflate the badge and
        // hand them back on the next get_stack.
        let repo = Repo::new("queue");
        let mut store = Store::default();
        dispatch(
            &mut store,
            repo.path(),
            None,
            &ToolCall::new(
                "mark_passage",
                json!({"file": "docs/a.md", "quote": "the first paragraph", "note": "a finding"}),
            ),
        )
        .expect("ok");

        let stack = dispatch(
            &mut store,
            repo.path(),
            None,
            &ToolCall::new("get_stack", json!({})),
        )
        .expect("ok");
        assert!(
            stack.as_array().unwrap().is_empty(),
            "the agent's note reached the human's queue: {stack}"
        );
        // ...but it is on the document, where it belongs.
        let marks = dispatch(
            &mut store,
            repo.path(),
            None,
            &ToolCall::new("list_highlights", json!({})),
        )
        .expect("ok");
        assert_eq!(marks.as_array().unwrap().len(), 1);
        assert_eq!(marks[0]["by"], "agent");
        assert_eq!(marks[0]["annotation"], "a finding");
    }

    #[test]
    fn mark_passage_refuses_a_path_outside_the_repo_root() {
        // Security-shaped. Break the `inside_root` call in `resolve_in_root`
        // and this must go red.
        let repo = Repo::new("outside");
        let outside = repo.path().parent().expect("a parent").join(format!(
            "dreamd-mcp-outside-target-{}.md",
            std::process::id()
        ));
        fs::write(&outside, "secret\n").expect("fixture");

        let mut store = Store::default();
        for file in [
            outside.to_string_lossy().into_owned(),
            "/etc/hosts".to_string(),
        ] {
            let err = dispatch(
                &mut store,
                repo.path(),
                None,
                &ToolCall::new(
                    "mark_passage",
                    json!({"file": file, "quote": "secret", "note": "n"}),
                ),
            )
            .unwrap_err();
            assert!(
                matches!(err, ToolError::Refused(_)),
                "{file} was not refused: {err:?}"
            );
        }
        assert!(
            store.parts().0.is_empty(),
            "a refused call must not have marked anything"
        );
        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn mark_passage_refuses_a_path_that_escapes_by_dot_dot() {
        // Distinct from the test above: this one pins that canonicalisation
        // happens *before* containment. `root.join("../x.md")` starts with the
        // root textually, so a check that skipped canonicalising would let it
        // straight through.
        let repo = Repo::new("dotdot");
        let escaped = repo.path().parent().expect("a parent").join(format!(
            "dreamd-mcp-dotdot-target-{}.md",
            std::process::id()
        ));
        fs::write(&escaped, "secret\n").expect("fixture");
        let name = escaped.file_name().unwrap().to_string_lossy().into_owned();

        let mut store = Store::default();
        for file in [
            format!("../{name}"),
            format!("docs/../../{name}"),
            format!("./docs/./../../{name}"),
        ] {
            let err = dispatch(
                &mut store,
                repo.path(),
                None,
                &ToolCall::new(
                    "mark_passage",
                    json!({"file": file, "quote": "secret", "note": "n"}),
                ),
            )
            .unwrap_err();
            assert!(
                matches!(err, ToolError::Refused(_)),
                "{file} escaped the root: {err:?}"
            );
        }
        assert!(store.parts().0.is_empty());
        let _ = fs::remove_file(&escaped);
    }

    #[test]
    fn mark_passage_refuses_a_quote_that_is_not_in_the_file() {
        // Better than anchoring at line 0 and letting the mark land silently at
        // the top of the document: the agent has the source and can retry.
        let repo = Repo::new("quote");
        let mut store = Store::default();
        let err = dispatch(
            &mut store,
            repo.path(),
            None,
            &ToolCall::new(
                "mark_passage",
                json!({"file": "docs/a.md", "quote": "nowhere in this file", "note": "n"}),
            ),
        )
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
        assert!(err.to_string().contains("verbatim"), "{err}");
        assert!(store.parts().0.is_empty());
    }

    #[test]
    fn mark_passage_requires_all_three_arguments() {
        let repo = Repo::new("args");
        let mut store = Store::default();
        for args in [
            json!({"quote": "q", "note": "n"}),
            json!({"file": "docs/a.md", "note": "n"}),
            json!({"file": "docs/a.md", "quote": "q"}),
        ] {
            let err = dispatch(
                &mut store,
                repo.path(),
                None,
                &ToolCall::new("mark_passage", args),
            )
            .unwrap_err();
            assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
        }
    }

    // ---- the tools/call envelope ------------------------------------------

    #[test]
    fn a_successful_call_carries_both_a_text_and_a_structured_copy() {
        let (mut store, ids) = store_with(&[("alpha", Some("why?"))]);
        let v = call(
            &mut store,
            Path::new("/repo"),
            None,
            &ToolCall::new("get_highlight", json!({"id": ids[0]})),
        );
        assert_eq!(v["isError"], false);
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["structuredContent"]["id"], ids[0]);

        // `get_stack` answers with an array, which cannot be structured
        // content; the text copy is still complete.
        let v = call(
            &mut store,
            Path::new("/repo"),
            None,
            &ToolCall::new("get_stack", json!({})),
        );
        assert_eq!(v["isError"], false);
        assert!(v.get("structuredContent").is_none(), "{v}");
        assert!(v["content"][0]["text"].as_str().unwrap().contains("why?"));
    }
}
