//! Send the selected highlight/annotation stack to an agent in one action.
//!
//! Delivery, resolved automatically (tmux is optional — it only upgrades the
//! experience to zero-paste):
//!   1. tmux present + a pane running `claude` (or a configured target) ->
//!      write the query to a temp file and `send-keys` a FIXED, dreamd-authored
//!      command into that pane. User content never enters the command string.
//!   2. otherwise -> copy the query to the clipboard (+ keep the temp file) so
//!      the user can paste it into Claude.ai / the desktop app / any agent.

use crate::annotations::{HighlightState, Pair};
use crate::config::Config;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
pub struct SendResult {
    /// "tmux" or "clipboard".
    pub method: String,
    /// Human-readable detail for a toast (e.g. the pane, or "copied").
    pub detail: String,
    /// Path to the temp query file (kept around for reference).
    pub temp_path: String,
}

/// Build the query markdown from the selected pairs.
pub fn assemble_query(repo_root: &Path, pairs: &[Pair]) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "# dreamd query\n\nRepo root: `{}`\n\n",
        repo_root.display()
    );
    out.push_str(
        "The following are highlighted passages (evidence) from markdown in this \
         repo, each paired with a question/comment. Answer each, grounding \
         against the project you already have context on.\n\n",
    );
    for (i, p) in pairs.iter().enumerate() {
        let h = &p.highlight;
        let rel = Path::new(&h.file_path)
            .strip_prefix(repo_root)
            .map(|r| r.to_string_lossy().into_owned())
            .unwrap_or_else(|_| h.file_path.clone());
        let _ = write!(out, "## {}. {rel}:{}", i + 1, h.line_start);
        if h.line_end != h.line_start {
            let _ = write!(out, "-{}", h.line_end);
        }
        out.push_str("\n\n");
        if h.state == HighlightState::Stale {
            out.push_str(
                "> _(note: this highlight is stale — the source text may have changed)_\n\n",
            );
        }
        out.push_str("**Evidence:**\n\n");
        for line in h.quote.lines() {
            out.push_str("> ");
            out.push_str(line);
            out.push('\n');
        }
        let _ = write!(out, "\n**Question:** {}\n\n", p.annotation);
    }
    out
}

fn write_temp(content: &str) -> std::io::Result<PathBuf> {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("dreamd-query-{}-{}.md", std::process::id(), n);
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, content)?;
    Ok(path)
}

fn tmux_available() -> bool {
    Command::new("tmux")
        .args(["list-panes", "-a"])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Find a tmux pane whose running command or title mentions `claude`.
fn detect_claude_pane() -> Option<String> {
    let out = Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{pane_id}\t#{pane_current_command}\t#{pane_title}",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().find_map(|line| {
        // "<pane_id>\t<current_command>\t<title>"
        let (id, rest) = line.split_once('\t')?;
        let matches = !id.is_empty() && rest.to_lowercase().contains("claude");
        matches.then(|| id.to_string())
    })
}

fn tmux_run(args: &[&str], what: &str) -> std::io::Result<()> {
    if Command::new("tmux").args(args).status()?.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("tmux {what} failed")))
    }
}

fn tmux_send(pane: &str, temp_path: &Path) -> std::io::Result<()> {
    // Fixed, dreamd-authored prompt. The only variable is the temp path, and it
    // is passed as a single argv entry (no shell), so nothing is injectable.
    let prompt = format!(
        "Please read @{} and respond to each question, grounding against this repo.",
        temp_path.display()
    );
    tmux_run(
        &["send-keys", "-t", pane, "-l", &prompt],
        "send-keys (text)",
    )?;
    tmux_run(&["send-keys", "-t", pane, "Enter"], "send-keys (Enter)")
}

/// Copy `content` to the system clipboard. Also the fallback delivery path.
pub fn copy_clipboard(content: &str) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_text(content).map_err(|e| e.to_string())
}

/// One-button send. Resolves delivery automatically.
pub fn send(config: &Config, repo_root: &Path, pairs: &[Pair]) -> Result<SendResult, String> {
    if pairs.is_empty() {
        return Err("Nothing selected to send.".into());
    }
    let content = assemble_query(repo_root, pairs);
    let temp_path = write_temp(&content).map_err(|e| format!("temp file: {e}"))?;
    let temp_str = temp_path.to_string_lossy().into_owned();

    // Explicit configured target wins.
    let target = config.tmux_target.clone().or_else(|| {
        if config.tmux_autodetect && tmux_available() {
            detect_claude_pane()
        } else {
            None
        }
    });

    if let Some(pane) = target {
        match tmux_send(&pane, &temp_path) {
            Ok(()) => {
                return Ok(SendResult {
                    method: "tmux".into(),
                    detail: format!("sent to pane {pane}"),
                    temp_path: temp_str,
                })
            }
            // Fall through to clipboard on any tmux failure.
            Err(e) => eprintln!("dreamd: tmux send failed ({e}), falling back to clipboard"),
        }
    }

    copy_clipboard(&content)?;
    Ok(SendResult {
        method: "clipboard".into(),
        detail: "copied to clipboard — paste into Claude".into(),
        temp_path: temp_str,
    })
}
