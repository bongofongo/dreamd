//! Repo scan for markdown files using the `ignore` crate (ripgrep's walker),
//! which respects `.gitignore`/`.ignore` automatically. Produces a nested
//! `FileNode` tree for the frontend.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct FileNode {
    /// File or directory name (leaf component).
    pub name: String,
    /// Absolute path.
    pub path: String,
    /// Path relative to the repo root (stable id the frontend can key on).
    pub rel: String,
    pub is_dir: bool,
    pub children: Vec<FileNode>,
}

fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md") | Some("markdown") | Some("mdown") | Some("mkd")
    )
}

/// Walk `repo_root` and return a directory tree containing only markdown files
/// (and the directories leading to them). `extra_ignores` are added as
/// overrides on top of `.gitignore`.
pub fn scan(repo_root: &Path, extra_ignores: &[String]) -> FileNode {
    // Collect markdown file paths, respecting ignore files.
    let mut builder = ignore::WalkBuilder::new(repo_root);
    builder.hidden(false); // show dotfiles like READMEs in dot-dirs unless ignored
    builder.git_global(true).git_ignore(true).git_exclude(true);

    if !extra_ignores.is_empty() {
        let mut ov = ignore::overrides::OverrideBuilder::new(repo_root);
        for pat in extra_ignores {
            // `!pat` in override means "ignore this".
            let _ = ov.add(&format!("!{pat}"));
        }
        if let Ok(ov) = ov.build() {
            builder.overrides(ov);
        }
    }

    let mut files: Vec<PathBuf> = Vec::new();
    for entry in builder.build().flatten() {
        let p = entry.path();
        if p.is_file() && is_markdown(p) {
            files.push(p.to_path_buf());
        }
    }
    files.sort();

    build_tree(repo_root, &files)
}

/// Assemble a nested tree from a flat list of file paths.
fn build_tree(repo_root: &Path, files: &[PathBuf]) -> FileNode {
    // Intermediate mutable tree keyed by component name.
    #[derive(Default)]
    struct Dir {
        dirs: BTreeMap<String, Dir>,
        files: BTreeMap<String, PathBuf>,
    }

    let mut root = Dir::default();
    for f in files {
        let rel = f.strip_prefix(repo_root).unwrap_or(f);
        let comps: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if comps.is_empty() {
            continue;
        }
        let mut cur = &mut root;
        for comp in &comps[..comps.len() - 1] {
            cur = cur.dirs.entry(comp.clone()).or_default();
        }
        let leaf = comps.last().unwrap().clone();
        cur.files.insert(leaf, f.clone());
    }

    fn to_node(name: &str, dir: &Dir, repo_root: &Path, abs: &Path) -> FileNode {
        let mut children = Vec::new();
        for (dname, d) in &dir.dirs {
            let child_abs = abs.join(dname);
            children.push(to_node(dname, d, repo_root, &child_abs));
        }
        for (fname, fpath) in &dir.files {
            let rel = fpath
                .strip_prefix(repo_root)
                .unwrap_or(fpath)
                .to_string_lossy()
                .into_owned();
            children.push(FileNode {
                name: fname.clone(),
                path: fpath.to_string_lossy().into_owned(),
                rel,
                is_dir: false,
                children: Vec::new(),
            });
        }
        let rel = abs
            .strip_prefix(repo_root)
            .unwrap_or(abs)
            .to_string_lossy()
            .into_owned();
        FileNode {
            name: name.to_string(),
            path: abs.to_string_lossy().into_owned(),
            rel,
            is_dir: true,
            children,
        }
    }

    to_node(
        &repo_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| repo_root.to_string_lossy().into_owned()),
        &root,
        repo_root,
        repo_root,
    )
}

/// Flat list of all markdown file paths (used to build the fuzzy index).
pub fn markdown_paths(repo_root: &Path, extra_ignores: &[String]) -> Vec<PathBuf> {
    let mut builder = ignore::WalkBuilder::new(repo_root);
    builder.hidden(false);
    if !extra_ignores.is_empty() {
        let mut ov = ignore::overrides::OverrideBuilder::new(repo_root);
        for pat in extra_ignores {
            let _ = ov.add(&format!("!{pat}"));
        }
        if let Ok(ov) = ov.build() {
            builder.overrides(ov);
        }
    }
    let mut files: Vec<PathBuf> = builder
        .build()
        .flatten()
        .map(|e| e.into_path())
        .filter(|p| p.is_file() && is_markdown(p))
        .collect();
    files.sort();
    files
}
