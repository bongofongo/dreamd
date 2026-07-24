//! Repo traversal.
//!
//! This matters twice over. It runs synchronously in `main()` before the window
//! exists, and it runs *again* via `list_markdown_files` as the frontend boots —
//! so a cold start pays for it twice. Worse, every `file-added`/`file-removed`
//! watcher event triggers `rebuild_index` **and** `loadTree`, which is another
//! two full walks per event, undebounced.
//!
//! `markdown_paths` and `scan` are benched separately so the tree-building cost
//! is visible on its own — `scan` is now `build_tree(markdown_paths(..))`, so
//! the difference between the two groups *is* `build_tree`. Fix B4 is still
//! open: it's about the **startup pair** below, where `main()` and
//! `list_markdown_files` each walk the repo, not about the two functions.

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

/// What a cold start actually does today: walk for the index, then walk again
/// for the tree. Kept as its own number so fix B4 has a single figure to move.
fn bench_startup_walks(c: &mut Criterion) {
    let mut g = c.benchmark_group("walk/startup_pair");
    // Fast operations, but many of them; the default 100 samples adds
    // minutes across the sweep for precision this doesn't need.
    g.sample_size(20);
    for n in common::REPO_SIZES {
        let root = common::repo(*n);
        g.bench_with_input(BenchmarkId::from_parameter(n), &root, |b, root| {
            b.iter(|| {
                black_box(fs_walk::markdown_paths(black_box(root), &[]));
                black_box(fs_walk::scan(black_box(root), &[]));
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
