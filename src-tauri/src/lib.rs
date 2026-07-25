//! dreamd's internals, exposed as a library so benches and tests can drive them
//! directly. The binary (`src/main.rs`) is a thin shell: CLI parsing, `AppState`,
//! the Tauri commands, and the builder. Everything those commands actually *do*
//! lives here.
//!
//! Splitting this out is what makes `src-tauri/benches/` possible — a `[[bin]]`
//! target cannot be imported.

pub mod annotations;
pub mod catalog;
pub mod cli;
pub mod config;
pub mod fs_walk;
pub mod markdown;
#[cfg(target_os = "macos")]
pub mod menu;
pub mod perf;
pub mod search;
pub mod send;
pub mod theme;
pub mod watcher;

use std::path::{Path, PathBuf};

/// Extensions we treat as markdown.
pub fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md" | "markdown" | "mdown" | "mkd")
    )
}

/// Walk up from `arg` (or the cwd) looking for a `.git`, and use that as the
/// tree root. Falls back to the starting directory when there is no repo.
pub fn resolve_repo_root(arg: Option<PathBuf>) -> PathBuf {
    resolve_repo_root_found(arg).0
}

/// `resolve_repo_root`, but reporting whether a `.git` was actually found.
///
/// The bare version cannot express the difference between "the repo is at
/// `start`" and "there is no repo, have `start` anyway", and that difference is
/// load-bearing: a `.app` launched from Finder gets cwd `/` from LaunchServices,
/// so the fallback would hand `/` to the walker as a tree root and dreamd would
/// try to index the entire filesystem before its window exists.
pub fn resolve_repo_root_found(arg: Option<PathBuf>) -> (PathBuf, bool) {
    let start =
        arg.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let start = start.canonicalize().unwrap_or(start);
    for dir in start.ancestors() {
        if dir.join(".git").exists() {
            return (dir.to_path_buf(), true);
        }
    }
    (start, false)
}

/// Resolve the CLI argument into a (tree root, file-to-open, has-repo) triple.
/// - a file arg  -> root = repo of the current directory; open the file if markdown.
/// - a dir arg   -> root = repo of that directory; nothing pre-opened.
/// - no arg      -> root = repo of the current directory.
///
/// `has_repo` is false only when nothing was asked for and nothing was found —
/// an explicit path is always honoured, repo or not, because someone who types
/// `dreamd ~/notes` means it. See `main` for what a false does.
pub fn resolve_target(arg: Option<PathBuf>) -> (PathBuf, Option<String>, bool) {
    match arg {
        Some(p) => {
            let abs = p.canonicalize().unwrap_or(p);
            if abs.is_file() {
                // The tree is rooted at the cwd's repo, not the file's — so a
                // file opened from outside a repo still opens, with an empty
                // tree beside it rather than a walk of wherever we happen to be.
                let (root, found) = resolve_repo_root_found(None);
                let file = is_markdown(&abs).then(|| abs.to_string_lossy().into_owned());
                (root, file, found)
            } else {
                (resolve_repo_root(Some(abs)), None, true)
            }
        }
        None => {
            let (root, found) = resolve_repo_root_found(None);
            (root, None, found)
        }
    }
}

pub fn read_source(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))
}

/// Render a path home-relative (`~/foo` instead of `/Users/me/foo`).
pub fn home_relative(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.to_string_lossy());
        }
    }
    path.to_string_lossy().into_owned()
}
