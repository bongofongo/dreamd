//! The root path field: what a typed line means, and the directory listing
//! behind its tab completion.
//!
//! This lives in a module rather than in the two commands for the same reason
//! `guard` does — `main.rs` is a `[[bin]]` and cannot be imported, so a rule
//! enforced there is a rule no test can reach.
//!
//! `guard::inside_root` deliberately does **not** apply here: leaving the
//! current root is the entire point of the field. What stands in its place is
//! narrower, and is the whole of what this surface may do:
//!
//! - the path must be absolute once `~` is expanded, so nothing is ever
//!   resolved against the process's cwd (which is `/` for a Finder launch);
//! - only an existing directory can be listed, so a file is never opened;
//! - only *directory names* come back — never a full path, never a file name,
//!   so this cannot become a filesystem browser for repo content;
//! - a dot-directory appears only when the typed prefix asked for one by name;
//! - and the listing is capped, so a directory with a hundred thousand entries
//!   costs one screenful rather than an IPC message the size of the directory.

use std::path::{Path, PathBuf};

/// How many completions come back at most. A completion list is read by a
/// human, and everything past the first screenful is weight on the wire.
pub const MAX_ENTRIES: usize = 200;

/// What a typed line means as a path, or why it cannot mean one.
///
/// `~` is expanded against `home`; `~user` is not, because resolving another
/// account's home is a different question and answering it badly would be a
/// silent wrong answer rather than an error.
pub fn resolve(input: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let text = input.trim();
    if text.is_empty() {
        return Err("type a path".into());
    }
    let path = if text == "~" || text.starts_with("~/") {
        let home = home.ok_or_else(|| "cannot find your home directory".to_string())?;
        match text.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => home.to_path_buf(),
        }
    } else if text.starts_with('~') {
        return Err("~user paths are not supported — type the path out".into());
    } else {
        PathBuf::from(text)
    };
    if !path.is_absolute() {
        return Err(format!("{text} is not an absolute path"));
    }
    Ok(path)
}

/// The path `set_root` should adopt: resolved, and known to exist.
///
/// A file is allowed through — `adopt_root` roots the tree at the file's repo
/// and opens the file when it is markdown, which is what File ▸ Open does with
/// a picked file and the least surprising thing to do with a pasted one.
pub fn target(input: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let path = resolve(input, home)?;
    if !path.exists() {
        return Err(format!("no such path: {}", path.display()));
    }
    Ok(path)
}

/// The directory children of what has been typed so far, as bare names.
///
/// The line is split at its last `/`: everything before it is the directory to
/// list, everything after is the prefix to match. Matching is case-insensitive
/// because the filesystem this ships on usually is.
pub fn complete(input: &str, home: Option<&Path>) -> Result<Vec<String>, String> {
    let (dir_text, prefix) = split(input.trim());
    let dir = resolve(dir_text, home)?;
    // `is_dir` follows symlinks, which is right: a symlinked directory is a
    // directory to anyone typing a path at it.
    if !dir.is_dir() {
        return Err(format!("no directory {}", dir.display()));
    }
    let entries =
        std::fs::read_dir(&dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;

    // A dot-directory is listed only when the prefix names one. Not a security
    // boundary — the reader can type `~/.config` and get it — but a completion
    // that offers `.git`, `.cache` and `.DS_Store` before the directory the
    // reader is looking for is a worse tool.
    let hidden_wanted = prefix.starts_with('.');
    let prefix = prefix.to_lowercase();

    let mut names: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && !hidden_wanted {
            continue;
        }
        if !name.to_lowercase().starts_with(&prefix) {
            continue;
        }
        if !entry.path().is_dir() {
            continue;
        }
        names.push(name);
    }
    names.sort();
    names.truncate(MAX_ENTRIES);
    Ok(names)
}

/// Split a typed line into the directory to list and the prefix to match.
///
/// With no `/` at all there is nothing to list *from*: the whole line is taken
/// as the directory, which then fails `resolve`'s absolute-path rule — the one
/// case where a relative fragment could otherwise have been answered against
/// the process's cwd.
fn split(input: &str) -> (&str, &str) {
    match input.rfind('/') {
        Some(i) => (&input[..=i], &input[i + 1..]),
        None => (input, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch tree, named after the test so the suite can run in parallel.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dreamd-rootfield-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn home() -> PathBuf {
        PathBuf::from("/home/reader")
    }

    #[test]
    fn tilde_expands_against_home() {
        let h = home();
        assert_eq!(resolve("~", Some(&h)).unwrap(), h);
        assert_eq!(resolve("~/notes", Some(&h)).unwrap(), h.join("notes"));
        assert_eq!(resolve("~/notes/", Some(&h)).unwrap(), h.join("notes"));
    }

    #[test]
    fn another_users_home_is_refused_rather_than_guessed() {
        let err = resolve("~someone/notes", Some(&home())).unwrap_err();
        assert!(err.contains("~user"), "got {err:?}");
    }

    #[test]
    fn a_relative_path_is_refused() {
        // The rule that keeps the process cwd — `/` for a Finder launch — out
        // of every answer this module gives.
        for text in ["notes", "./notes", "../notes", "notes/deep"] {
            let err = resolve(text, Some(&home())).unwrap_err();
            assert!(err.contains("not an absolute path"), "{text}: {err:?}");
        }
        let err = complete("notes/", Some(&home())).unwrap_err();
        assert!(err.contains("not an absolute path"), "got {err:?}");
    }

    #[test]
    fn an_empty_line_is_not_a_path() {
        assert!(resolve("   ", Some(&home())).is_err());
    }

    #[test]
    fn complete_lists_directories_and_nothing_else() {
        let dir = scratch("dirs-only");
        std::fs::create_dir_all(dir.join("alpha")).unwrap();
        std::fs::create_dir_all(dir.join("beta")).unwrap();
        std::fs::write(dir.join("secrets.txt"), "shh").unwrap();
        std::fs::write(dir.join("alpha.md"), "# no").unwrap();

        let got = complete(&format!("{}/", dir.display()), None).unwrap();
        assert_eq!(got, vec!["alpha".to_string(), "beta".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn complete_returns_names_never_paths() {
        // The difference between a completion list and a directory browser: a
        // name says a child exists, a path is a handle to it.
        let dir = scratch("names-only");
        std::fs::create_dir_all(dir.join("alpha")).unwrap();

        let got = complete(&format!("{}/", dir.display()), None).unwrap();
        assert_eq!(got, vec!["alpha".to_string()]);
        assert!(
            got.iter().all(|n| !n.contains('/')),
            "a name must not carry a path: {got:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_prefix_narrows_the_listing_case_insensitively() {
        let dir = scratch("prefix");
        std::fs::create_dir_all(dir.join("Notes")).unwrap();
        std::fs::create_dir_all(dir.join("nothing")).unwrap();
        std::fs::create_dir_all(dir.join("other")).unwrap();

        let got = complete(&format!("{}/no", dir.display()), None).unwrap();
        assert_eq!(got, vec!["Notes".to_string(), "nothing".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hidden_directories_are_offered_only_when_asked_for() {
        let dir = scratch("hidden");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("visible")).unwrap();

        let bare = complete(&format!("{}/", dir.display()), None).unwrap();
        assert_eq!(bare, vec!["visible".to_string()], "unasked-for dotfiles");

        let asked = complete(&format!("{}/.", dir.display()), None).unwrap();
        assert_eq!(asked, vec![".git".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_cannot_be_listed() {
        let dir = scratch("file-target");
        let file = dir.join("notes.md");
        std::fs::write(&file, "# hi").unwrap();

        let err = complete(&format!("{}/", file.display()), None).unwrap_err();
        assert!(err.contains("no directory"), "got {err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_listing_is_capped() {
        let dir = scratch("cap");
        for i in 0..(MAX_ENTRIES + 50) {
            std::fs::create_dir_all(dir.join(format!("d{i:04}"))).unwrap();
        }
        let got = complete(&format!("{}/", dir.display()), None).unwrap();
        assert_eq!(got.len(), MAX_ENTRIES);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn target_must_exist() {
        let dir = scratch("target");
        assert_eq!(target(&dir.display().to_string(), None).unwrap(), dir);

        let err = target(&format!("{}/gone", dir.display()), None).unwrap_err();
        assert!(err.contains("no such path"), "got {err:?}");

        // A file is allowed through: `adopt_root` roots at its repo and opens
        // it when it is markdown.
        let file = dir.join("notes.md");
        std::fs::write(&file, "# hi").unwrap();
        assert_eq!(target(&file.display().to_string(), None).unwrap(), file);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn splitting_happens_at_the_last_slash() {
        assert_eq!(split("/a/b/c"), ("/a/b/", "c"));
        assert_eq!(split("/a/b/"), ("/a/b/", ""));
        assert_eq!(split("/"), ("/", ""));
        assert_eq!(split("~/"), ("~/", ""));
        assert_eq!(split("~"), ("~", ""));
    }
}
