---
name: perf-quick
description: Runs the ~60-90s quick performance tier on the dreamd repo — regenerates the corpus if needed, runs reduced-sample criterion benches over the render/locate/search paths, runs the Chromium render and scroll scenarios, and reports only the metrics that moved beyond noise. Invoke after a meaningful edit to src-tauri/ or ui/, when you want a fast read on whether a change cost anything, or runs /perf-quick.
---

# Perf quick

The fast performance check for the `dreamd` repo (root = the directory containing
this `.claude/` folder, currently `/Users/oliverfong/toadmountain/dreamd/`). Meant to
be run by you, mid-thread, right after a change that could plausibly affect speed —
not by the user, and not as a gate.

It deliberately trades precision for latency: reduced criterion sample counts, a
subset of the corpus, no app launch, no release build. It will catch a gross
regression. It will **not** resolve a 5% move, and you must not claim it did.

Run this after edits to `src-tauri/src/markdown.rs`, `search.rs`, `fs_walk.rs`,
`annotations.rs`, or to the render/scroll/highlight paths in `ui/app.js`. Skip it for
docs, skills, comments, and anything in `send.rs` or `config.rs` — say you skipped it
and why rather than burning 90 seconds proving nothing changed.

## 1. Run the tier

From the repo root:

```sh
./perf/run.sh quick
```

It regenerates the corpus if stale (usually a no-op), runs the benches, runs the two
Chromium scenarios, writes `perf/results/quick-<sha>-<stamp>.json`, and prints a diff
against `perf/baseline.json`.

Expect roughly 45–90 seconds. If it takes materially longer, something rebuilt —
`Cargo.toml` changes force a full bench-profile rebuild, which is a one-off. Say so
rather than reporting the inflated wall time as the tier's cost.

## 2. Read the output

The table only lists metrics that moved beyond their source's noise floor. `XX`
marks a regression, `!` a smaller slowdown, `+` an improvement, `*` a metric with no
baseline entry. The command exits non-zero only on `XX`.

Thresholds differ by where the number came from, because the noise floors do —
measured, and recorded in `perf/README.md`. Rust benches are judged at 5/15%;
Chromium rows at 20/35%, since a whole browser engine sampled once drifts up to 27%
between runs on unchanged code. Don't tighten either without re-measuring the floor
first: a threshold under the noise reports a regression every other run.

This tier runs the *same* criterion settings as the slower ones and simply covers
fewer benchmarks. If you ever make it cheaper by cutting sample counts, its numbers
stop being comparable to the baseline and it will report phantom regressions.

Rows are grouped by where the number came from, and the distinction matters:

- **RUST BENCHES** and **REAL APP** are real measurements of real code.
- **CHROMIUM HARNESS** rows come from Playwright's Chromium with a stubbed
  `window.__TAURI__`, **not** from WKWebView. They are trustworthy for detecting
  that something got slower, and worthless as absolute figures. Whenever you quote
  one, say it is Chromium-relative.

If the run reports `no baseline at perf/baseline.json`, stop and tell the user — the
tier measured fine but has nothing to compare against, and the fix is a
`./perf/run.sh deep --update-baseline`, which is theirs to authorize.

## 3. Interpret, don't just relay

A single quick run at `--sample-size 10` is noisy. Before calling anything a
regression:

- Re-run once. Noise rarely reproduces; a real regression always does.
- Check the change plausibly touches that path. A render-bench move after a pure
  `send.rs` edit is noise, not a finding.
- Ignore movement in metrics the tier didn't really exercise.

Do **not** update `perf/baseline.json`, and do **not** commit. This skill measures;
it does not decide. If a regression is real, say what moved, by how much, and which
edit is the likely cause — then let the user choose.

## 4. Report

Two or three sentences. What you ran, what moved, and whether you believe it. If
nothing moved beyond noise, say exactly that — "quick tier clean, nothing beyond
±5%" — and move on. Do not paste the whole table; it is in the results file, and the
path is in the run output.
