//! Fuzzy palette latency.
//!
//! `ui/app.js` wires `pin.oninput` straight to `fuzzy_search` with no debounce,
//! so this runs once per keystroke on the main thread. `SearchIndex::query`
//! also constructs a fresh `Matcher::new(Config::DEFAULT)` and re-parses the
//! pattern every call (search.rs:65), which is what the `keystrokes` group is
//! shaped to expose: it walks a query character by character, exactly as typing
//! does, rather than timing one query in isolation.

mod common;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use dreamd::fs_walk;
use dreamd::search::SearchIndex;
use std::hint::black_box;

fn index_for(n: usize) -> (std::path::PathBuf, SearchIndex) {
    let root = common::repo(n);
    let paths = fs_walk::markdown_paths(&root, &[]);
    let index = SearchIndex::build(&root, &paths);
    (root, index)
}

fn bench_build(c: &mut Criterion) {
    let mut g = c.benchmark_group("index_build");
    // Fast operations, but many of them; the default 100 samples adds
    // minutes across the sweep for precision this doesn't need.
    g.sample_size(20);
    for n in common::REPO_SIZES {
        let root = common::repo(*n);
        let paths = fs_walk::markdown_paths(&root, &[]);
        g.bench_with_input(BenchmarkId::from_parameter(n), &paths, |b, paths| {
            b.iter(|| black_box(SearchIndex::build(black_box(&root), black_box(paths))))
        });
    }
    g.finish();
}

fn bench_query(c: &mut Criterion) {
    let mut g = c.benchmark_group("query");
    // Fast operations, but many of them; the default 100 samples adds
    // minutes across the sweep for precision this doesn't need.
    g.sample_size(20);
    for n in common::REPO_SIZES {
        let (_root, index) = index_for(*n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &index, |b, index| {
            b.iter(|| black_box(index.query(black_box("docs"))))
        });
    }
    g.finish();
}

/// One query per character of "notes", the way typing actually drives it.
/// Short prefixes are the expensive case: they match nearly the whole index,
/// and nucleo sorts every match before we `take(200)`.
fn bench_keystrokes(c: &mut Criterion) {
    const TYPED: &str = "notes";
    let mut g = c.benchmark_group("keystrokes");
    // Fast operations, but many of them; the default 100 samples adds
    // minutes across the sweep for precision this doesn't need.
    g.sample_size(20);
    for n in common::REPO_SIZES {
        let (_root, index) = index_for(*n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &index, |b, index| {
            b.iter(|| {
                for i in 1..=TYPED.len() {
                    black_box(index.query(black_box(&TYPED[..i])));
                }
            })
        });
    }
    g.finish();
}

criterion_group!(benches, bench_build, bench_query, bench_keystrokes);
criterion_main!(benches);
