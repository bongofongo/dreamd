//! The native agent surface: dreamd talking to Claude Code over NDJSON instead
//! of drawing its terminal.
//!
//! The embedded pane (see [`crate::pty`]) runs the real Claude Code TUI inside
//! xterm.js. It works, and everything the reader sees in it belongs to another
//! program — its palette, its typography, its scrollback, a composer dreamd
//! cannot see into. This module is the same agent reached a different way:
//! `claude -p --output-format stream-json --input-format stream-json`, whose
//! output is structure rather than pixels, so the conversation can be drawn in
//! dreamd's own palette with dreamd's own markdown pipeline.
//!
//! The layout mirrors [`crate::mcp`]'s, for the same reason and with the same
//! discipline — **nothing under here mentions `AppHandle`, `AppState` or any
//! other Tauri type**:
//!
//! - [`wire`] — pure. Another program's JSON, reduced to the events a pane
//!   draws. Lenient by contract; the drift lands here and nowhere else.
//! - [`claude`] — the child process: where `claude` is, what it is launched
//!   with, the reader thread, and the two lines dreamd ever writes to it.
//! - [`gate`] — pure policy plus the cards waiting on the reader. Deny is the
//!   answer to every kind of silence.
//! - [`gate_server`] — the per-session socket the permission hook talks to, and
//!   the `--settings` document that points it there.
//! - [`hook`] — `dreamd approve`, the process Claude Code spawns per tool call.
//!   Shaped like `mcp::shim`, but failing closed where that one fails open.
//!
//! [`Sink`] is shaped exactly like [`crate::pty::Sink`] and
//! [`crate::notify::Notifier`], and for the third time for the same reason: a
//! closure rather than a handle is what lets a harness drive the whole surface
//! with no window and no event loop. `main.rs` supplies the one closure that
//! knows a webview exists.

pub mod claude;
pub mod gate;
pub mod gate_server;
pub mod hook;
pub mod wire;

use std::sync::Arc;

pub use wire::AgentEvent;

/// Where events go. See the module docs for why this is a closure.
pub type Sink = Arc<dyn Fn(AgentEvent) + Send + Sync>;

/// One running conversation, whoever is on the other end of it.
///
/// There is exactly one implementation ([`claude::ClaudeSession`]) and this
/// trait is not an attempt to guess what a second one would need. It exists
/// because `AppState` has to name the type it stores, and naming a concrete
/// Claude Code session there would put "which agent" into the shape of every
/// command that touches it. Three methods is the whole surface the commands
/// use, so a second agent is a new file rather than a refactor of `main.rs`.
///
/// What is deliberately *not* here: spawning. Every agent will want different
/// arguments, and a constructor in a trait would either take the union of them
/// or take a bag of strings. `main.rs` matches on the configured agent once, at
/// launch, and holds a `Box<dyn Session>` afterwards.
pub trait Session: Send {
    /// Send one turn. `&self` rather than `&mut self` because an interrupt can
    /// arrive from another thread while a turn is being written.
    fn send(&self, text: &str) -> Result<(), String>;

    /// End the current turn, keeping the conversation.
    fn interrupt(&self) -> Result<(), String>;

    /// End the conversation. Must be idempotent.
    fn kill(&mut self);
}
