//! Repo scan for markdown files using the `ignore` crate (ripgrep's walker),
//! which respects `.gitignore`/`.ignore` automatically. Produces a nested
//! `FileNode` tree for the frontend.

use crate::is_markdown;
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

/// Walk `repo_root` and return a directory tree containing only markdown files
/// (and the directories leading to them). `extra_ignores` are added as
/// overrides on top of `.gitignore`.
pub fn scan(repo_root: &Path, extra_ignores: &[String]) -> FileNode {
    build_tree(repo_root, &markdown_paths(repo_root, extra_ignores))
}

/// Intermediate mutable tree keyed by component name, built before the
/// immutable `FileNode` tree the frontend sees.
#[derive(Default)]
struct Dir {
    dirs: BTreeMap<String, Dir>,
    files: BTreeMap<String, PathBuf>,
}

fn rel_of(path: &Path, repo_root: &Path) -> String {
    path.strip_prefix(repo_root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn to_node(name: &str, dir: &Dir, repo_root: &Path, abs: &Path) -> FileNode {
    let children = dir
        .dirs
        .iter()
        .map(|(dname, d)| to_node(dname, d, repo_root, &abs.join(dname)))
        .chain(dir.files.iter().map(|(fname, fpath)| FileNode {
            name: fname.clone(),
            path: fpath.to_string_lossy().into_owned(),
            rel: rel_of(fpath, repo_root),
            is_dir: false,
            children: Vec::new(),
        }))
        .collect();
    FileNode {
        name: name.to_string(),
        path: abs.to_string_lossy().into_owned(),
        rel: rel_of(abs, repo_root),
        is_dir: true,
        children,
    }
}

/// Assemble a nested tree from a flat list of file paths.
fn build_tree(repo_root: &Path, files: &[PathBuf]) -> FileNode {
    let mut root = Dir::default();
    for f in files {
        let comps: Vec<String> = f
            .strip_prefix(repo_root)
            .unwrap_or(f)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let Some((leaf, parents)) = comps.split_last() else {
            continue;
        };
        let mut cur = &mut root;
        for comp in parents {
            cur = cur.dirs.entry(comp.clone()).or_default();
        }
        cur.files.insert(leaf.clone(), f.clone());
    }

    let name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo_root.to_string_lossy().into_owned());
    to_node(&name, &root, repo_root, repo_root)
}

/// Flat list of all markdown file paths, sorted. `.gitignore`/`.ignore` are
/// respected by the walker itself; `extra_ignores` are layered on as overrides
/// (`!pat` in override syntax means "ignore this").
pub fn markdown_paths(repo_root: &Path, extra_ignores: &[String]) -> Vec<PathBuf> {
    let mut builder = ignore::WalkBuilder::new(repo_root);
    builder.hidden(false); // show dotfiles like READMEs in dot-dirs unless ignored
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
