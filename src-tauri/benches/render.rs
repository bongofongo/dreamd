//! Markdown render cost — the hot path behind every file open and every
//! watcher-triggered re-render.
//!
//! Two things are measured separately on purpose:
//!
//!   `syntect_cold`  the one-time flate2+bincode load of syntect's bundled
//!                   syntaxes and themes. Still lazy (`OnceLock` in
//!                   markdown.rs), so the *first document paint* pays it; this
//!                   group is what moving it to a background thread at startup
//!                   would be worth.
//!
//!   `render`        steady-state parse + emit, with syntect **and the
//!                   code-block memo** already warm. Mixing in the dump load
//!                   would make every render number a lie.
//!
//! **`render/code/*` and `render/mixed/*` overstate the code memo enormously —
//! do not read them as a speed-up.** `markdown::highlight_blocks` memoizes
//! highlighted fences process-wide on `(theme, lang, code)`, and criterion runs
//! the same document thousands of times, so from the second iteration on every
//! fence is a hit and syntect never runs at all. Worse, the corpus repeats
//! itself: the generated documents draw on **32 distinct code blocks**, so even
//! the first iteration of a 640-block document highlights 32 of them. What
//! these groups measure now is parse, memo lookup and `push_html` — real work,
//! and a genuine regression in it still shows up here, but the ratio to the
//! pre-memo numbers is an artefact of the corpus.
//!
//! The honest measure of the memo is `perf/scripts/loop.sh`'s `save_to_paint`:
//! a real edit to a real file, where exactly one block changed and the rest
//! were highlighted in some earlier render rather than in this iteration.
//! `markdown::clear_code_cache()` is available if a bench ever wants the cold
//! path back.

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
