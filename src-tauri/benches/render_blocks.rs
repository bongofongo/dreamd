//! The render the *reader* pays for.
//!
//! `benches/render.rs` measures `markdown::render`, which is what
//! `render_agent_text` calls; every document opened or saved goes through
//! `markdown::render_blocks`, which is the same parse and the same `push_html`
//! plus a boundary per top-level block. The gap between the two is what that
//! split costs.
//!
//! **A separate target on purpose.** Put in the same binary as `render`, this
//! group moved `render/table/2m` by 12% and `render/mixed/2m` by 9% — on code
//! that had not changed a byte, and reproducibly: removing the group put both
//! back. Two criterion groups in one binary perturb each other's layout that
//! much, and a headline metric that moves when an unrelated benchmark is added
//! beside it is a metric nobody can read.

mod common;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use dreamd::markdown;
use std::hint::black_box;

/// Force the `OnceLock`s in markdown.rs to initialize before timing anything.
fn warm() {
    let _ = markdown::render("```rust\nfn main() {}\n```\n");
}

/// One variant is enough to see the split's cost; four sizes keep the shape
/// visible.
fn bench_render_blocks(c: &mut Criterion) {
    warm();

    let mut g = c.benchmark_group("render_blocks");
    for size in common::SIZES {
        let src = common::doc("mixed", size);
        g.sample_size(match *size {
            "2m" | "8m" | "512k" => 10,
            "128k" => 30,
            _ => 100,
        });
        g.throughput(Throughput::Bytes(src.len() as u64));
        g.bench_with_input(BenchmarkId::new("mixed", *size), &src, |b, src| {
            b.iter(|| {
                black_box(markdown::render_blocks(
                    black_box(src),
                    markdown::CODE_THEME,
                ))
            })
        });
    }
    g.finish();
}

criterion_group!(benches, bench_render_blocks);
criterion_main!(benches);
