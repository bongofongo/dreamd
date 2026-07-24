//! Telescope-style fuzzy finder over markdown file paths, backed by `nucleo`
//! (the matcher Helix uses). v1 indexes paths only; in-file heading / content
//! search is deferred to v2.
//!
//! Note: this reproduces Telescope's *feel* — real Telescope is a Neovim Lua
//! plugin and cannot render inside a webview.

use crate::fs_walk::FileNode;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

struct Entry {
    name: String,
    path: String,
    rel: String,
}

pub struct SearchIndex {
    entries: Vec<Entry>,
    by_rel: HashMap<String, usize>,
}

impl SearchIndex {
    pub fn build(repo_root: &Path, paths: &[PathBuf]) -> Self {
        let mut entries = Vec::with_capacity(paths.len());
        let mut by_rel = HashMap::new();
        for p in paths {
            let rel = p
                .strip_prefix(repo_root)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned();
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            by_rel.insert(rel.clone(), entries.len());
            entries.push(Entry {
                name,
                path: p.to_string_lossy().into_owned(),
                rel,
            });
        }
        Self { entries, by_rel }
    }

    fn node(&self, e: &Entry) -> FileNode {
        FileNode {
            name: e.name.clone(),
            path: e.path.clone(),
            rel: e.rel.clone(),
            is_dir: false,
            children: Vec::new(),
        }
    }

    /// Ranked fuzzy matches. Empty query returns the first slice of the index.
    pub fn query(&self, q: &str) -> Vec<FileNode> {
        const LIMIT: usize = 200;
        if q.trim().is_empty() {
            return self.entries.iter().take(LIMIT).map(|e| self.node(e)).collect();
        }
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(q, CaseMatching::Smart, Normalization::Smart);
        let items = self.entries.iter().map(|e| e.rel.as_str());
        let matches = pattern.match_list(items, &mut matcher);
        matches
            .into_iter()
            .take(LIMIT)
            .filter_map(|(rel, _score)| self.by_rel.get(rel).map(|&i| self.node(&self.entries[i])))
            .collect()
    }
}
