//! Correctness check for `markdown::locate` against the corpus highlight set.
//!
//!     cargo run --release --example locate_check [-v]
//!
//! The repo has no `#[cfg(test)]` tests; this is the standing check for the one
//! piece of logic where correctness is genuinely subtle. The invariant:
//!
//!   For every sampled highlight, `locate` must resolve the **rendered**
//!   (whitespace-collapsed) string the frontend actually sends back to the
//!   lines the quote was sampled from.
//!
//! Three columns per fixture:
//!
//!   `truth`  where the generator took the quote from, recorded in the fixture.
//!            It is not recoverable by searching — the corpus repeats whole
//!            blocks verbatim (one sampled code block occurs 195 times), so
//!            `source.find(quote)` lands on the first copy, which is usually
//!            not the right one. An earlier version of this check used
//!            `source.find` as its ground truth and was wrong about which
//!            answers were wrong.
//!   `bare`   the rendered quote with empty context — what the app sent before
//!            highlights carried context. Whitespace-normalized tier 3 takes
//!            the first occurrence, so every repeated quote anchors to the
//!            wrong copy.
//!   `ctx`    the rendered quote with rendered prefix/suffix — what the app
//!            sends now.
//!
//! When a mismatch lands on a copy that is *indistinguishable* — the stripped
//! quote appears there with the stripped context matching in full, exactly as
//! it does at `truth` — it is reported AMBIGUOUS rather than WRONG. No anchor
//! built from quote plus context can tell those apart, and only WRONG rows fail
//! the run.
//!
//! It also asserts that `Store::reanchor_file` — which shares one `SourceIndex`
//! across every highlight in a file — agrees with one-shot `markdown::locate`.
//!
//! An example rather than a bin: it never becomes part of the shipped binary.

#[path = "../benches/common.rs"]
mod common;

use dreamd::annotations::{HighlightState, Store};
use dreamd::markdown::{self, Location};

const FILE: &str = "/corpus/mixed-2m.md";
const COUNTS: &[usize] = &[1, 10, 100, 500];

/// Collapse runs of whitespace, the way the rendered DOM does. The frontend can
/// only ever supply a quote or context in this form.
fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

fn show(loc: &Option<Location>) -> String {
    match loc {
        Some(l) => format!("{},{}", l.line_start, l.line_end),
        None => "none,none".to_string(),
    }
}

/// The document with whitespace removed, plus line lookup. Deliberately a
/// second, independent implementation of what `markdown.rs` does internally —
/// a checker that reuses the code under test cannot catch it being wrong.
struct Doc {
    stripped: String,
    /// Source byte offset for each byte of `stripped`.
    offsets: Vec<usize>,
    line_starts: Vec<usize>,
}

impl Doc {
    fn new(source: &str) -> Self {
        let mut stripped = String::with_capacity(source.len());
        let mut offsets = Vec::with_capacity(source.len());
        for (i, ch) in source.char_indices() {
            if !ch.is_whitespace() {
                stripped.push(ch);
                offsets.resize(stripped.len(), i);
            }
        }
        let mut line_starts = vec![0];
        line_starts.extend(source.match_indices('\n').map(|(i, _)| i + 1));
        Self {
            stripped,
            offsets,
            line_starts,
        }
    }

    fn line_at(&self, byte_idx: usize) -> usize {
        self.line_starts.partition_point(|&s| s <= byte_idx)
    }

    /// Every line span where `quote_ns` occurs with `prefix_ns`/`suffix_ns`
    /// matching in full — i.e. every answer that is defensible on the evidence
    /// an anchor carries.
    fn perfect_spans(&self, quote_ns: &str, prefix_ns: &str, suffix_ns: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        // Overlapping occurrences included: the corpus is periodic in places,
        // and `str::match_indices` cannot see a match that begins inside the
        // previous one.
        let mut from = 0;
        while let Some(off) = self.stripped.get(from..).and_then(|s| s.find(quote_ns)) {
            let at = from + off;
            from = at + 1;
            while from < self.stripped.len() && !self.stripped.is_char_boundary(from) {
                from += 1;
            }
            let before = &self.stripped.as_bytes()[..at];
            let after = &self.stripped.as_bytes()[at + quote_ns.len()..];
            let pre_ok = before.len() >= prefix_ns.len() && before.ends_with(prefix_ns.as_bytes());
            let suf_ok = after.starts_with(suffix_ns.as_bytes());
            if pre_ok && suf_ok {
                let last = at + quote_ns.len().saturating_sub(1);
                out.push((
                    self.line_at(self.offsets[at]),
                    self.line_at(self.offsets[last]),
                ));
            }
        }
        out
    }
}

fn main() {
    let source = common::doc("mixed", "2m");
    let doc = Doc::new(&source);
    let verbose = std::env::args().any(|a| a == "-v" || a == "--verbose");

    let mut total = 0usize;
    let mut bare_bad = 0usize;
    let mut ctx_wrong = 0usize;
    let mut ctx_ambiguous = 0usize;
    let mut reanchor_bad = 0usize;
    let mut unresolved = 0usize;
    let mut disagreements = 0usize;

    println!("n,index,truth_start,truth_end,bare_start,bare_end,ctx_start,ctx_end,verdict");

    for &n in COUNTS {
        let fixtures = common::highlights(n);

        // What the frontend sends: a rendered quote plus rendered context. The
        // store is seeded at the true lines, so re-anchoring is the realistic
        // scenario — a highlight that was anchored correctly, and a save.
        let mut store = Store::default();
        for f in &fixtures {
            store.add_highlight(
                FILE.to_string(),
                f.line_start,
                f.line_end,
                f.rendered.clone(),
                collapse(&f.prefix),
                collapse(&f.suffix),
            );
        }
        let reanchored = store.reanchor_file(FILE, &source);

        for (i, f) in fixtures.iter().enumerate() {
            total += 1;
            let (p, s) = (collapse(&f.prefix), collapse(&f.suffix));
            let truth = (f.line_start, f.line_end);
            let bare = markdown::locate(&source, "", &f.rendered, "");
            let ctx = markdown::locate(&source, &p, &f.rendered, &s);

            let at = |l: &Option<Location>| l.as_ref().map(|l| (l.line_start, l.line_end));
            let bare_ok = at(&bare) == Some(truth);
            let ctx_ok = at(&ctx) == Some(truth);
            if !bare_ok {
                bare_bad += 1;
            }
            if ctx.is_none() {
                unresolved += 1;
            }

            // A mismatch is only a real failure if a distinguishable copy
            // existed and `locate` picked the wrong one.
            let mut ambiguous = false;
            if !ctx_ok {
                let spans = doc.perfect_spans(&strip_ws(&f.rendered), &strip_ws(&p), &strip_ws(&s));
                ambiguous = spans.len() > 1
                    && spans.contains(&truth)
                    && at(&ctx).is_some_and(|g| spans.contains(&g));
                if ambiguous {
                    ctx_ambiguous += 1;
                } else {
                    ctx_wrong += 1;
                }
            }

            if !ctx_ok || verbose {
                println!(
                    "{n},{i},{},{},{},{},{}",
                    truth.0,
                    truth.1,
                    show(&bare),
                    show(&ctx),
                    match (ctx_ok, ambiguous) {
                        (true, _) => "ok",
                        (false, true) => "ambiguous",
                        (false, false) => "WRONG",
                    }
                );
            }

            // Re-anchoring: the highlight was at `truth`, the file was saved
            // unchanged, so it must still be at `truth`. Unlike first-time
            // anchoring this has a position hint, which is what settles the
            // copies that quote and context cannot.
            let h = &reanchored[i];
            let batched = match h.state {
                HighlightState::Active => Some((h.line_start, h.line_end)),
                HighlightState::Stale => None,
            };
            if batched != Some(truth) {
                reanchor_bad += 1;
                println!("  reanchor moved {n}/{i}: {truth:?} -> {batched:?}");
            }

            // `reanchor_file` shares one SourceIndex across every highlight in
            // the file; one-shot `locate_near` builds a fresh one per call.
            // They must agree.
            let one_shot = markdown::SourceIndex::new(&source)
                .locate_near(&p, &f.rendered, &s, f.line_start)
                .map(|l| (l.line_start, l.line_end));
            if batched != one_shot {
                disagreements += 1;
                println!(
                    "  reanchor disagreement at {n}/{i}: batched {batched:?} vs one-shot {one_shot:?}"
                );
            }
        }
    }

    println!();
    println!("fixtures:                        {total}");
    println!("first anchor, empty context:     {bare_bad} wrong");
    println!("first anchor, with context:      {ctx_wrong} wrong, {ctx_ambiguous} ambiguous");
    println!("re-anchor (context + position):  {reanchor_bad} moved");
    println!("unresolved (None):               {unresolved}");
    println!("batched vs one-shot:             {disagreements} disagreements");

    if ctx_wrong > 0 || reanchor_bad > 0 || unresolved > 0 || disagreements > 0 {
        std::process::exit(1);
    }
}
