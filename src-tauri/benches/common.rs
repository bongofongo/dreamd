//! Shared corpus loading for the benches.
//!
//! Not a bench target itself — `autobenches = false` in Cargo.toml means only
//! the explicitly declared `[[bench]]` entries are built, so this file is free
//! to be a plain `mod common;` include.

#![allow(dead_code)] // each bench uses a different subset

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const VARIANTS: &[&str] = &["prose", "code", "table", "mixed"];
pub const SIZES: &[&str] = &["8k", "128k", "512k", "2m"];
pub const REPO_SIZES: &[usize] = &[10, 500, 5000];

pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../perf/corpus/generated")
}

fn missing(what: &str) -> ! {
    panic!(
        "corpus file missing: {what}\n\
         run `node perf/corpus/gen.mjs` from the repo root first \
         (or just use `./perf/run.sh`, which does it for you)"
    );
}

/// One document from the corpus, e.g. `doc("mixed", "2m")`.
pub fn doc(variant: &str, size: &str) -> String {
    let p = corpus_dir().join("docs").join(format!("{variant}-{size}.md"));
    std::fs::read_to_string(&p).unwrap_or_else(|_| missing(&p.display().to_string()))
}

/// A sampled highlight, as written by `perf/corpus/gen.mjs`.
#[derive(Deserialize, Clone)]
pub struct Fixture {
    /// The exact source slice — hits `locate`'s cheap first/second tier.
    pub quote: String,
    /// The same text with whitespace collapsed, approximating what
    /// `getSelection().toString()` returns from the rendered DOM. This is what
    /// the app actually sends, and it falls through to `locate_normalized`.
    pub rendered: String,
    pub prefix: String,
    pub suffix: String,
}

pub fn highlights(n: usize) -> Vec<Fixture> {
    let p = corpus_dir().join("highlights").join(format!("{n}.json"));
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|_| missing(&p.display().to_string()));
    serde_json::from_str(&raw).expect("corpus highlight fixture is not valid JSON")
}

pub fn repo(n: usize) -> PathBuf {
    let p = corpus_dir().join("repos").join(format!("repo-{n}"));
    if !p.is_dir() {
        missing(&p.display().to_string());
    }
    p
}
