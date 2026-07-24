//! Repo traversal.
//!
//! This runs synchronously in `main()` before the window exists, and again on
//! every `file-added`/`file-removed` watcher event via `rebuild_index`.
//!
//! `markdown_paths` and `scan` are benched separately so the tree-building cost
//! is visible on its own — `scan` is `build_tree(markdown_paths(..))`, so the
//! difference between the two groups *is* `build_tree`.
//!
//! The cold start used to walk the repo **twice**: once in `main()` for the
//! search index, then again via `list_markdown_files` when the frontend booted.
//! `main` now builds the tree from the paths it already has and caches it in
//! `AppState`, so `startup_pair` below is one walk plus one `build_tree`. The
//! group keeps its name so the number stays comparable across that change.

mod common;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use dreamd::fs_walk;
use std::hint::black_box;

fn bench_markdown_paths(c: &mut Criterion) {
    let mut g = c.benchmark_group("walk/markdown_paths");
    // Fast operations, but many of them; the default 100 samples adds
    // minutes across the sweep for precision this doesn't need.
    g.sample_size(20);
    for n in common::REPO_SIZES {
        let root = common::repo(*n);
        g.throughput(Throughput::Elements(*n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(n), &root, |b, root| {
            b.iter(|| black_box(fs_walk::markdown_paths(black_box(root), &[])))
        });
    }
    g.finish();
}

fn bench_scan(c: &mut Criterion) {
    let mut g = c.benchmark_group("walk/scan");
    // Fast operations, but many of them; the default 100 samples adds
    // minutes across the sweep for precision this doesn't need.
    g.sample_size(20);
    for n in common::REPO_SIZES {
        let root = common::repo(*n);
        g.throughput(Throughput::Elements(*n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(n), &root, |b, root| {
            b.iter(|| black_box(fs_walk::scan(black_box(root), &[])))
        });
    }
    g.finish();
}

/// What a cold start actually does: one walk, feeding both the search index and
/// the cached tree.
fn bench_startup_walks(c: &mut Criterion) {
    let mut g = c.benchmark_group("walk/startup_pair");
    // Fast operations, but many of them; the default 100 samples adds
    // minutes across the sweep for precision this doesn't need.
    g.sample_size(20);
    for n in common::REPO_SIZES {
        let root = common::repo(*n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &root, |b, root| {
            b.iter(|| {
                let paths = black_box(fs_walk::markdown_paths(black_box(root), &[]));
                black_box(fs_walk::build_tree(black_box(root), &paths));
            })
        });
    }
    g.finish();
}

criterion_group!(
    benches,
    bench_markdown_paths,
    bench_scan,
    bench_startup_walks
);
criterion_main!(benches);
