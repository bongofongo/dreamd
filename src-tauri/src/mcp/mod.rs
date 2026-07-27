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
//!
//! **The flow this surface exists to serve is queue-first.** The agent's entry
//! point is `get_stack` — the human's ordered queue of annotated passages — not
//! a file listing. dreamd is not a document server an agent browses; it is the
//! human's outbox, and this is the half where the outbox answers back.

pub mod jsonrpc;
pub mod schema;
pub mod server;
pub mod shim;
pub mod tools;
pub mod view;
