//! The MCP server's protocol core: pure, and deliberately Tauri-free.
//!
//! Nothing under here mentions `AppHandle`, `AppState`, `Window` or any other
//! Tauri type. [`tools::dispatch`] takes a `&mut Store` and the repo root and
//! nothing else, which is what lets the whole tool surface be driven by the
//! same `Store` fixtures `annotations.rs` already uses — no window, no event
//! loop, no socket. The socket, the thread and the Tauri wiring live outside
//! this module (step 3c) and call in.
//!
//! The layout mirrors the four concerns:
//!
//! - [`jsonrpc`] — request/response/error types and the NDJSON framing.
//! - [`schema`] — the tool list, as a compiled-in `const &str` of JSON. Both
//!   halves of the binary (the GUI's server and the `dreamd mcp` stdio shim)
//!   read this one const, so `tools/list` cannot drift from what dispatch
//!   actually implements.
//! - [`tools`] — dispatch.
//! - [`view`] — the wire DTOs, and the single choke point where document text
//!   is handed to `untrusted::delimit` on its way out.
//!
//! **The flow this surface exists to serve is queue-first.** The agent's entry
//! point is `get_stack` — the human's ordered queue of annotated passages — not
//! a file listing. dreamd is not a document server an agent browses; it is the
//! human's outbox, and this is the half where the outbox answers back.

pub mod jsonrpc;
pub mod schema;
pub mod tools;
pub mod view;
