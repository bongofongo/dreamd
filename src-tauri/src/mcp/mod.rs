//! dreamd's MCP server: the protocol core, the socket, and the stdio shim.
//!
//! Nothing under here mentions `AppHandle`, `AppState`, `Window` or any other
//! Tauri type. [`tools::dispatch`] takes a `&mut Store` and the repo root and
//! nothing else, and [`server::spawn`] takes a `notify::Notifier` closure
//! rather than a handle — which is what lets the whole surface, transport
//! included, be driven with no window and no event loop (`examples/mcp_check.rs`
//! does exactly that). `main.rs` supplies the closure and owns the one line
//! that knows a webview exists.
//!
//! The layout mirrors the concerns:
//!
//! - [`jsonrpc`] — request/response/error types and the NDJSON framing.
//! - [`schema`] — the tool list, as a compiled-in `const &str` of JSON. Both
//!   halves of the binary (the GUI's server and the `dreamd mcp` stdio shim)
//!   read this one const, so `tools/list` cannot drift from what dispatch
//!   actually implements.
//! - [`tools`] — dispatch.
//! - [`view`] — the wire DTOs, and the single choke point where document text
//!   is handed to `untrusted::delimit` on its way out.
//! - [`server`] — the Unix socket the GUI listens on, and the method routing.
//!   Binding it is also how a dreamd claims a repo.
//! - [`shim`] — `dreamd mcp`, the process Claude Code spawns. Answers
//!   `initialize`/`tools/list` locally and proxies `tools/call` to the socket.
//! - [`register`] — the `--mcp-config` document dreamd hands its own agent, and
//!   the `claude mcp add dreamd` line printed for the surfaces it does not
//!   launch.
//!
//! **The flow this surface exists to serve is queue-first.** The agent's entry
//! point is `get_stack` — the human's ordered queue of annotated passages — not
//! a file listing. dreamd is not a document server an agent browses; it is the
//! human's outbox, and this is the half where the outbox answers back.

use std::path::PathBuf;
use std::sync::Arc;

pub mod jsonrpc;
pub mod register;
pub mod schema;
pub mod server;
pub mod shim;
pub mod tools;
pub mod view;

/// Where to ask what the human currently has on screen.
///
/// A closure for the same reason [`crate::notify::Notifier`] is one, and it is
/// the second half of the same bargain: the answer lives in `AppState`, which
/// nothing under this module may name, so `main.rs` supplies a reader and the
/// tool layer receives a plain `PathBuf`. That is what keeps `get_open_document`
/// drivable from `mcp_check` with no window — the harness passes a closure over
/// a fixture.
///
/// Returns an **absolute** path, or `None` when no document is open. Validating
/// it is the tool's job, not this closure's: see [`tools`].
pub type OpenDoc = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;

/// A reader that always says "nothing open".
///
/// The counterpart to [`crate::notify::discard`], for a server with no window
/// behind it — and the honest answer in that case, since there is no reader and
/// therefore nothing they are reading.
pub fn no_open_doc() -> OpenDoc {
    Arc::new(|| None)
}

/// A reader that always names the same file. For harnesses.
pub fn fixed_open_doc(path: impl Into<PathBuf>) -> OpenDoc {
    let path = path.into();
    Arc::new(move || Some(path.clone()))
}
