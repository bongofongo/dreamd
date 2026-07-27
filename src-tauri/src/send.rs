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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotations::Highlight;

    fn pair(quote: &str, annotation: &str, line_start: usize, line_end: usize) -> Pair {
        Pair {
            highlight: Highlight {
                id: 1,
                file_path: "/repo/docs/a.md".into(),
                line_start,
                line_end,
                quote: quote.into(),
                prefix: String::new(),
                suffix: String::new(),
                state: HighlightState::Active,
                annotation: Some(annotation.into()),
            },
            annotation: annotation.into(),
        }
    }

    fn query(pairs: &[Pair]) -> String {
        assemble_query(Path::new("/repo"), pairs)
    }

    #[test]
    fn the_header_names_the_repo_root() {
        let out = query(&[]);
        assert!(out.starts_with("# dreamd query\n"), "{out:?}");
        assert!(out.contains("Repo root: `/repo`"), "{out:?}");
    }

    #[test]
    fn paths_are_reported_relative_to_the_repo_root() {
        let out = query(&[pair("evidence", "why?", 4, 4)]);
        assert!(out.contains("## 1. docs/a.md:4\n"), "{out}");
        assert!(
            !out.contains("/repo/docs/a.md"),
            "absolute path leaked: {out}"
        );
    }

    #[test]
    fn a_path_outside_the_repo_root_is_left_absolute() {
        let mut p = pair("evidence", "why?", 1, 1);
        p.highlight.file_path = "/elsewhere/b.md".into();
        assert!(
            query(&[p]).contains("## 1. /elsewhere/b.md:1"),
            "not absolute"
        );
    }

    #[test]
    fn a_single_line_highlight_reports_one_line_number() {
        let out = query(&[pair("evidence", "why?", 7, 7)]);
        assert!(out.contains("docs/a.md:7\n"), "{out}");
        assert!(!out.contains("7-7"), "{out}");
    }

    #[test]
    fn a_multi_line_highlight_reports_a_range() {
        let out = query(&[pair("evidence", "why?", 7, 9)]);
        assert!(out.contains("docs/a.md:7-9"), "{out}");
    }

    #[test]
    fn pairs_are_numbered_in_stack_order() {
        let out = query(&[
            pair("first", "q1", 1, 1),
            pair("second", "q2", 2, 2),
            pair("third", "q3", 3, 3),
        ]);
        let at = |needle: &str| {
            out.find(needle)
                .unwrap_or_else(|| panic!("{needle} missing"))
        };
        assert!(at("## 1. ") < at("## 2. "));
        assert!(at("## 2. ") < at("## 3. "));
        assert!(at("q1") < at("q2") && at("q2") < at("q3"));
    }

    #[test]
    fn every_evidence_line_is_block_quoted() {
        // Otherwise a quote containing a `#` or a `-` reads as the agent's own
        // markdown structure rather than as evidence.
        let out = query(&[pair("# not a heading\n- not a list", "why?", 1, 2)]);
        assert!(out.contains("> # not a heading\n"), "{out}");
        assert!(out.contains("> - not a list\n"), "{out}");
    }

    #[test]
    fn a_stale_highlight_is_labelled_as_such() {
        let mut p = pair("evidence", "why?", 1, 1);
        p.highlight.state = HighlightState::Stale;
        assert!(query(&[p]).contains("this highlight is stale"));
        // ...and an active one carries no note.
        assert!(!query(&[pair("evidence", "why?", 1, 1)]).contains("stale"));
    }

    #[test]
    fn the_annotation_is_carried_verbatim() {
        let out = query(&[pair("evidence", "Why `this` and not $that?", 1, 1)]);
        assert!(
            out.contains("**Question:** Why `this` and not $that?"),
            "{out}"
        );
    }

    #[test]
    fn nothing_a_user_wrote_can_reach_a_command_line() {
        // Tenet 3, the assertable half: the query is a document. `send` writes
        // it to a temp file and tmux receives a fixed `read @<path>` prompt, so
        // the shell never sees any of this — but assert the assembler itself
        // does no escaping, quoting or interpolation of its own that would
        // suggest otherwise.
        let nasty = "$(rm -rf /) `id` \"; echo pwned; #";
        let out = query(&[pair(nasty, nasty, 1, 1)]);
        assert!(out.contains(&format!("> {nasty}\n")), "{out}");
        assert!(out.contains(&format!("**Question:** {nasty}")), "{out}");
    }
}
