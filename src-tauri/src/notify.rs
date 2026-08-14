//! The one event dreamd pushes at the frontend on its own initiative.
//!
//! Every other repaint in the app is pulled: a command returns, and its return
//! value *is* the frontend's source of truth for the mutation it just asked
//! for. That is how the ~54 commands work, and it is why they do not emit — a
//! second signal would mean two repaint paths racing, which `refreshStack`'s
//! `stackSeq` guard already exists to survive once. The one command that does
//! is `set_root`, and it proves the rule: `adopt_root` finishes the re-walk on
//! a background thread long after the command has returned, so there is no
//! return value left to carry the answer and `repo-changed` is emitted instead.
//!
//! The MCP layer is the exception, and the only one: it mutates the store from
//! a socket thread, on behalf of an agent the window never heard from. Without
//! a push, an agent resolving six marks would be invisible until the user
//! pressed a key. **So `marks-changed` is emitted from the MCP layer and
//! nowhere else** — in particular not from a Tauri command, which keeps the
//! `save_to_paint` loop entirely out of this.
//!
//! [`Notifier`] is a plain boxed closure rather than an `AppHandle` so the
//! socket server can be built and driven — by `mcp_check`, and by anything
//! else — with no window and no event loop. [`to_window`] is the only place
//! that knows Tauri exists.

use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

/// The event name the frontend listens for (`ui/app.js`'s `onMarksChanged`).
pub const EVENT: &str = "marks-changed";

/// What changed. Both fields are hints that let the frontend do the least
/// work, not a description of the mutation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MarksChanged {
    /// The file whose overlay is now wrong, if it is just the one. `None` means
    /// "assume the open document" — the safe answer, and the one to reach for
    /// when the affected path cannot be determined.
    pub file_path: Option<String>,
    /// Whether the send stack changed, and so whether the badge and the stack
    /// panel need refreshing.
    pub stack: bool,
}

impl MarksChanged {
    pub fn file(path: impl Into<String>, stack: bool) -> Self {
        Self {
            file_path: Some(path.into()),
            stack,
        }
    }
}

/// Somewhere to send a change.
///
/// `Arc<dyn Fn>` rather than `AppHandle` on purpose: it is what lets
/// `mcp::server` be exercised head-lessly, and it means a test can assert on
/// what *would* have been emitted instead of on a window.
pub type Notifier = Arc<dyn Fn(MarksChanged) + Send + Sync>;

/// The real one: emit to the webview.
pub fn to_window(app: AppHandle) -> Notifier {
    Arc::new(move |change| {
        // A closed window is not an error worth logging on every mutation.
        let _ = app.emit(EVENT, change);
    })
}

/// A notifier that drops everything, for a server running without a window.
pub fn discard() -> Notifier {
    Arc::new(|_| {})
}

/// A notifier that records what it was handed. For harnesses that need to
/// assert an event happened without a window to receive it.
pub fn recording() -> (Notifier, Arc<std::sync::Mutex<Vec<MarksChanged>>>) {
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = log.clone();
    (
        Arc::new(move |change| sink.lock().unwrap().push(change)),
        log,
    )
}
