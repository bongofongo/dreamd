//! Markdown render cost — the hot path behind every file open and every
//! watcher-triggered re-render.
//!
//! Two things are measured separately on purpose:
//!
//!   `syntect_cold`  the one-time flate2+bincode load of syntect's bundled
//!                   syntaxes and themes. Today this is lazy (`OnceLock` in
//!                   markdown.rs), so the *first document paint* pays it. Fix
//!                   B3 moves it to a background thread at startup; this group
//!                   is how we know what that's worth.
//!
//!   `render`        steady-state parse + highlight, with syntect already warm.
//!                   Mixing the two would make every render number a lie.

mod common;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use dreamd::markdown;
use std::hint::black_box;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

/// Force the `OnceLock`s in markdown.rs to initialize before timing anything.
fn warm() {
    let _ = markdown::render("```rust\nfn main() {}\n```\n");
}

fn bench_render(c: &mut Criterion) {
    warm();

    let mut g = c.benchmark_group("render");
    for variant in common::VARIANTS {
        for size in common::SIZES {
            let src = common::doc(variant, size);
            // A 2MB code-heavy document takes seconds to render, so the default
            // 100 samples would put this group alone past five minutes. Small
            // documents are cheap enough to keep a useful sample count.
            g.sample_size(match *size {
                "2m" | "8m" | "512k" => 10,
                "128k" => 30,
                _ => 100,
            });
            g.throughput(Throughput::Bytes(src.len() as u64));
            g.bench_with_input(BenchmarkId::new(*variant, *size), &src, |b, src| {
                b.iter(|| black_box(markdown::render(black_box(src))))
            });
        }
    }
    g.finish();
}

fn bench_syntect_cold(c: &mut Criterion) {
    let mut g = c.benchmark_group("syntect_cold");
    // Each iteration is a full dump load — expensive, so keep the sample count
    // low rather than letting criterion spend a minute on a constant.
    g.sample_size(10);

    g.bench_function("load_syntaxes", |b| {
        b.iter(|| black_box(SyntaxSet::load_defaults_newlines()))
    });
    g.bench_function("load_themes", |b| {
        b.iter(|| black_box(ThemeSet::load_defaults()))
    });
    g.finish();
}

criterion_group!(benches, bench_render, bench_syntect_cold);
criterion_main!(benches);
