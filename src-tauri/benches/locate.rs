//! Highlight re-anchoring — the cost paid on *every* save of the open file.
//!
//! `Store::reanchor_file` calls `markdown::locate` once per highlight in the
//! file. `locate`'s third tier, `locate_normalized`, builds a whitespace-
//! stripped copy of the entire source plus a `Vec<usize>` with one entry per
//! retained char — roughly 8 bytes per character — and throws it away again.
//! Per highlight. Against a 2MB document.
//!
//! Four scenarios, because which of `locate`'s three tiers you land in
//! dominates everything else:
//!
//!   `today`         a rendered (whitespace-collapsed) quote with **empty**
//!                   context. The name is historical: this is what the app sent
//!                   before highlights carried context, kept as the reference
//!                   the baseline was set against. Tiers 1 and 2 both miss and
//!                   every highlight pays for the whitespace-stripped tier.
//!   `exact_source`  the quote is a byte-exact source slice, so tier 2 hits
//!                   immediately. This is the floor — what `today` would cost
//!                   if anchoring were exact.
//!   `with_context`  rendered quote *plus* rendered context. This is what the
//!                   app sends now. Note what it does and does not buy: the
//!                   collapsed context still cannot match raw source, so tier 1
//!                   misses and the cost is roughly `today`'s. It was kept for
//!                   a while as proof that context is not a *speed* fix. It is
//!                   a correctness fix — without it a quote that appears twice
//!                   anchors to whichever copy comes first — and
//!                   `examples/locate_check.rs` is where that is measured.
//!   `stale`         the quote no longer exists, so all three tiers run to
//!                   completion before returning `None`.
//!
//! The headline: `today` and `stale` cost the same, because a rendered quote
//! never hits the cheap tiers. So the speed fix is not richer anchors — it is
//! building the whitespace-stripped index **once per `reanchor_file` call**
//! instead of once per highlight.

mod common;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use dreamd::annotations::Store;
use dreamd::markdown;
use std::hint::black_box;

const FILE: &str = "/corpus/mixed-2m.md";
const COUNTS: &[usize] = &[1, 10, 100, 500];

#[derive(Clone, Copy)]
#[allow(dead_code)] // WithContext is only used by locate_single
enum Mode {
    /// Rendered (whitespace-collapsed) quote, empty context — what the app sent
    /// before highlights carried context.
    Today,
    /// Byte-exact source quote — the tier-2 floor.
    ExactSource,
    /// Rendered quote plus *rendered* context — what the frontend sends.
    WithContext,
}

/// Collapse runs of whitespace, the way the rendered DOM does.
fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn store_with(n: usize, mode: Mode) -> Store {
    let mut store = Store::default();
    for h in common::highlights(n) {
        let (quote, prefix, suffix) = match mode {
            Mode::Today => (h.rendered.clone(), String::new(), String::new()),
            Mode::ExactSource => (h.quote.clone(), String::new(), String::new()),
            Mode::WithContext => (h.rendered.clone(), collapse(&h.prefix), collapse(&h.suffix)),
        };
        store.add_highlight(FILE.to_string(), 0, 0, quote, prefix, suffix);
    }
    store
}

fn bench_reanchor(c: &mut Criterion) {
    let source = common::doc("mixed", "2m");

    // `with_context` is deliberately absent from this sweep. At 500 highlights it
    // costs the same 4s/iteration as `today` — because it *is* the same path — so
    // running it here would double the group's wall time to re-measure the same
    // number. The cheap `locate_single/with_context` case below tracks it for a
    // few milliseconds instead, and the seeded save→repaint loop in `perf-pass`
    // covers it end to end.
    for (label, mode) in [("today", Mode::Today), ("exact_source", Mode::ExactSource)] {
        let mut g = c.benchmark_group(format!("reanchor/{label}"));
        // `reanchor/today/500` is ~4 SECONDS per iteration — that is the finding,
        // not a mistake. At criterion's default 100 samples this one group would
        // run for over ten minutes and blow the whole deep tier's budget, so the
        // sample count is capped. Precision suffers; the effect is so large that
        // it does not matter.
        g.sample_size(10);
        for n in COUNTS {
            g.bench_with_input(BenchmarkId::from_parameter(n), n, |b, &n| {
                b.iter_batched_ref(
                    || store_with(n, mode),
                    |store| black_box(store.reanchor_file(FILE, black_box(&source))),
                    criterion::BatchSize::SmallInput,
                )
            });
        }
        g.finish();
    }
}

/// `locate` on its own, one quote at a time — isolates the per-call cost from
/// the store's iteration and cloning.
fn bench_locate_single(c: &mut Criterion) {
    let source = common::doc("mixed", "2m");
    let fixture = common::highlights(1).remove(0);

    let mut g = c.benchmark_group("locate_single");
    g.bench_function("today", |b| {
        b.iter(|| {
            black_box(markdown::locate(
                black_box(&source),
                "",
                &fixture.rendered,
                "",
            ))
        })
    });
    g.bench_function("exact_source", |b| {
        b.iter(|| black_box(markdown::locate(black_box(&source), "", &fixture.quote, "")))
    });
    let (rp, rs) = (collapse(&fixture.prefix), collapse(&fixture.suffix));
    g.bench_function("with_context", |b| {
        b.iter(|| {
            black_box(markdown::locate(
                black_box(&source),
                &rp,
                &fixture.rendered,
                &rs,
            ))
        })
    });
    // The pathological case: a quote that no longer exists, so all three tiers
    // run to completion before returning None. This is what a stale highlight
    // costs, and every stale highlight pays it on every single save.
    g.bench_function("stale", |b| {
        b.iter(|| {
            black_box(markdown::locate(
                black_box(&source),
                "",
                "this text is not present anywhere in the corpus document",
                "",
            ))
        })
    });
    g.finish();
}

criterion_group!(benches, bench_reanchor, bench_locate_single);
criterion_main!(benches);
