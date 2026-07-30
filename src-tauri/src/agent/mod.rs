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
//!
//! [`Sink`] is shaped exactly like [`crate::pty::Sink`] and
//! [`crate::notify::Notifier`], and for the third time for the same reason: a
//! closure rather than a handle is what lets a harness drive the whole surface
//! with no window and no event loop. `main.rs` supplies the one closure that
//! knows a webview exists.

pub mod wire;

use std::sync::Arc;

pub use wire::AgentEvent;

/// Where events go. See the module docs for why this is a closure.
pub type Sink = Arc<dyn Fn(AgentEvent) + Send + Sync>;
