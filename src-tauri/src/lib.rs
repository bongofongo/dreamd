//! dreamd's internals, exposed as a library so benches and tests can drive them
//! directly. The binary (`src/main.rs`) is a thin shell: CLI parsing, `AppState`,
//! the Tauri commands, and the builder. Everything those commands actually *do*
//! lives here.
//!
//! Splitting this out is what makes `src-tauri/benches/` and `cargo test`
//! possible — a `[[bin]]` target cannot be imported. [`guard`] exists for that
//! reason alone: its two predicates decide what a document may reach, and while
//! they sat inline in `main.rs` nothing could assert on them.

pub mod annotations;
pub mod catalog;
pub mod cli;
pub mod config;
pub mod fs_walk;
pub mod guard;
pub mod markdown;
pub mod marks_file;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_extensions_are_recognised() {
        for name in ["a.md", "a.markdown", "a.mdown", "a.mkd", "deep/dir/a.md"] {
            assert!(is_markdown(Path::new(name)), "should accept {name}");
        }
    }

    #[test]
    fn near_miss_extensions_are_not_markdown() {
        // `.mdx` in particular: it is JSX, and rendering it as markdown would
        // silently escape half the file.
        for name in ["a.mdx", "a.txt", "a.rs", "a", "a.md.bak", "README"] {
            assert!(!is_markdown(Path::new(name)), "should refuse {name}");
        }
    }

    #[test]
    fn extension_matching_is_case_sensitive() {
        // Documented rather than desired: the walker and the frontend's own
        // regex agree on lowercase, so a `.MD` file is invisible to both.
        assert!(!is_markdown(Path::new("a.MD")));
    }

    #[test]
    fn a_directory_with_no_git_reports_no_repo() {
        // The false is what stops a Finder-launched `.app` — which gets cwd `/`
        // — handing `/` to the walker as a tree root.
        let dir = std::env::temp_dir().join(format!("dreamd-lib-test-{}", std::process::id()));
        let nested = dir.join("no").join("repo").join("here");
        std::fs::create_dir_all(&nested).expect("temp dirs");

        let (root, found) = resolve_repo_root_found(Some(nested.clone()));
        assert!(!found, "found a repo at {}", root.display());
        assert_eq!(root, nested.canonicalize().unwrap_or(nested.clone()));

        // Plant a `.git` two levels up and the walk stops there.
        let repo = dir.join("no");
        std::fs::create_dir_all(repo.join(".git")).expect("fake .git");
        let (root, found) = resolve_repo_root_found(Some(nested));
        assert!(found);
        assert_eq!(root, repo.canonicalize().unwrap_or(repo));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_explicit_directory_is_honoured_repo_or_not() {
        // Someone who types `dreamd ~/notes` means it, so `has_repo` is true
        // even when the walk found nothing.
        let dir = std::env::temp_dir().join(format!("dreamd-lib-arg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let (_, file, has_repo) = resolve_target(Some(dir.clone()));
        assert!(has_repo, "an explicit path must not disable the walker");
        assert_eq!(file, None, "a directory pre-opens nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_markdown_file_argument_opens_nothing() {
        let dir = std::env::temp_dir().join(format!("dreamd-lib-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let md = dir.join("a.md");
        let txt = dir.join("a.txt");
        std::fs::write(&md, "# hi\n").expect("write");
        std::fs::write(&txt, "hi\n").expect("write");

        let (_, file, _) = resolve_target(Some(md.clone()));
        assert!(file.is_some(), "a markdown file argument opens");
        let (_, file, _) = resolve_target(Some(txt));
        assert_eq!(file, None, "a non-markdown argument opens nothing");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn home_relative_only_rewrites_paths_under_home() {
        assert_eq!(
            home_relative(Path::new("/nowhere/near/home")),
            "/nowhere/near/home"
        );
        if let Some(home) = dirs::home_dir() {
            assert_eq!(home_relative(&home.join("notes")), "~/notes");
        }
    }

    #[test]
    fn read_source_reports_the_path_it_could_not_read() {
        let err = read_source("/definitely/not/a/file.md").unwrap_err();
        assert!(err.contains("/definitely/not/a/file.md"), "got {err:?}");
    }
}
