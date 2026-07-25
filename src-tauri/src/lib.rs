//! dreamd's internals, exposed as a library so benches and tests can drive them
//! directly. The binary (`src/main.rs`) is a thin shell: CLI parsing, `AppState`,
//! the Tauri commands, and the builder. Everything those commands actually *do*
//! lives here.
//!
//! Splitting this out is what makes `src-tauri/benches/` possible — a `[[bin]]`
//! target cannot be imported.

pub mod annotations;
pub mod cli;
pub mod config;
pub mod fs_walk;
pub mod markdown;
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
    let start =
        arg.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let start = start.canonicalize().unwrap_or(start);
    for dir in start.ancestors() {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
    }
    start
}

/// Resolve the CLI argument into a (tree root, file-to-open) pair.
/// - a file arg  -> root = repo of the current directory; open the file if markdown.
/// - a dir arg   -> root = repo of that directory; nothing pre-opened.
/// - no arg      -> root = repo of the current directory.
pub fn resolve_target(arg: Option<PathBuf>) -> (PathBuf, Option<String>) {
    match arg {
        Some(p) => {
            let abs = p.canonicalize().unwrap_or(p);
            if abs.is_file() {
                let root = resolve_repo_root(None);
                let file = is_markdown(&abs).then(|| abs.to_string_lossy().into_owned());
                (root, file)
            } else {
                (resolve_repo_root(Some(abs)), None)
            }
        }
        None => (resolve_repo_root(None), None),
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
