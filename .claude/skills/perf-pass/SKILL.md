---
name: perf-pass
description: Runs the ~5min full performance tier on the dreamd repo — the complete criterion sweep across every corpus variant and size, hyperfine cold-start timing, the save-to-repaint loop against the real app, peak RSS, and all four Chromium scenarios — then reports the diff against the committed baseline. Invoke at the end of a thread pass, before wrapping up a session that touched src-tauri/ or ui/, or runs /perf-pass.
---

# Perf pass

The full local performance run for the `dreamd` repo (root = the directory containing
this `.claude/` folder, currently `/Users/oliverfong/toadmountain/dreamd/`). This is
the tier whose numbers are worth quoting.

Unlike `perf-quick`, this one launches the real binary. A dreamd window will open and
close on its own — warn the user before you start, because an unexpected window
stealing focus mid-session is startling.

## 1. Gate first

Run `cargo build` from the repo root. A tier run against a broken tree wastes five
minutes. If the build fails, fix it or stop; do not "measure around" a compile error.

## 2. Run the tier

```sh
./perf/run.sh pass
```

Roughly five minutes. It runs, in order: corpus check, the complete criterion sweep
(`locate`, `render`, `search`, `walk` across every variant and size), the Chromium
scenarios at 2MB, `perf/scripts/startup.sh` (hyperfine over the pre-window sequence
plus a real launch-to-first-paint), and `perf/scripts/loop.sh` (the save-to-repaint
loop with 10 seeded highlights).

If the machine is doing something else heavy — a build, a video call — say so and
either wait or flag the numbers as contaminated. Do not quietly report noise.

On a headless or window-hostile environment, add `--no-window`. That skips
`loop.sh` and the first-paint measurement entirely; the run still succeeds, but it is
missing the single most product-relevant number, so say which parts were skipped.

## 3. Read the output

Same table and same grouping as `perf-quick`, over far more metrics. Two things
deserve specific attention every run:

- `real.loop.events_per_save` — how many `file-changed` events one atomic save
  produced. Anything above 1.0 means the document is being fully re-rendered more
  than once per `:w`. That is the missing watcher debounce, and it is the loop the
  whole product is built around.
- `real.loop.save_to_paint_ms.p95` — the tail of that same loop. This is what the
  user actually feels when they save in Neovim.

The `not measured this run` list at the bottom is not decoration. An entry there
means the baseline has a metric this run didn't produce — usually a skipped tier,
sometimes a script that failed silently. Investigate before concluding the run was
clean.

## 4. Interpret

Full sample counts, so a reproducible >15% move is a real regression rather than
noise. Still check it against what actually changed: attribute the move to a specific
edit, or say plainly that you cannot.

Do **not** update `perf/baseline.json` from this tier — the runner refuses anyway.
Baselines come from `perf-deep` only, because the pass tier does not build release.

## 5. Report

A short paragraph, then the handful of metrics that moved. Lead with the loop numbers
if they changed. State which rows are Chromium-relative. Name the results file path
once so the user can dig in, and don't paste the full table.

If a regression is real and the session is about to be committed, say so explicitly
and let the user decide — `wrap-up` will ask about it, but it should not be the first
time they hear it.
