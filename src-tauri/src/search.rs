//! Telescope-style fuzzy finder over markdown file paths, backed by `nucleo`
//! (the matcher Helix uses). v1 indexes paths only; in-file heading / content
//! search is deferred to v2.
//!
//! Note: this reproduces Telescope's *feel* — real Telescope is a Neovim Lua
//! plugin and cannot render inside a webview.

use crate::fs_walk::{rel_of, FileNode};
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher};
use std::path::{Path, PathBuf};

/// One indexed file. Matching is against `rel`, hence the `AsRef<str>` impl:
/// it lets `match_list` hand back the entry itself rather than a path string
/// we would then have to look up again.
struct Entry {
    name: String,
    path: String,
    rel: String,
}

impl AsRef<str> for Entry {
    fn as_ref(&self) -> &str {
        &self.rel
    }
}

impl From<&Entry> for FileNode {
    fn from(e: &Entry) -> Self {
        FileNode {
            name: e.name.clone(),
            path: e.path.clone(),
            rel: e.rel.clone(),
            is_dir: false,
            children: Vec::new(),
        }
    }
}

pub struct SearchIndex {
    entries: Vec<Entry>,
}

impl SearchIndex {
    pub fn build(repo_root: &Path, paths: &[PathBuf]) -> Self {
        let entries = paths
            .iter()
            .map(|p| Entry {
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: p.to_string_lossy().into_owned(),
                rel: rel_of(p, repo_root),
            })
            .collect();
        Self { entries }
    }

    /// Ranked fuzzy matches. Empty query returns the first slice of the index.
    pub fn query(&self, q: &str) -> Vec<FileNode> {
        const LIMIT: usize = 200;
        if q.trim().is_empty() {
            return self
                .entries
                .iter()
                .take(LIMIT)
                .map(FileNode::from)
                .collect();
        }
        // One matcher per thread, reused across keystrokes: nucleo's matcher
        // is built to be long-lived — its internal slabs grow while matching,
        // and a fresh one per query re-paid that growth over the whole entry
        // list every keystroke. Thread-local rather than a `Mutex` in the
        // index, which is otherwise pure; queries run on Tauri's bounded
        // blocking pool, so this is a handful of matchers, warm.
        thread_local! {
            static MATCHER: std::cell::RefCell<Matcher> =
                std::cell::RefCell::new(Matcher::new(Config::DEFAULT));
        }
        let pattern = Pattern::parse(q, CaseMatching::Smart, Normalization::Smart);
        MATCHER.with(|m| {
            pattern
                .match_list(&self.entries, &mut m.borrow_mut())
                .into_iter()
                .take(LIMIT)
                .map(|(e, _score)| FileNode::from(e))
                .collect()
        })
    }
}
