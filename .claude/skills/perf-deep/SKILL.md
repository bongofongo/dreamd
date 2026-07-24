---
name: perf-deep
description: Runs the ~20min deep performance tier on the dreamd repo — everything in perf-pass against both the debug and the release build, plus Instruments traces, a samply flamegraph, cargo-bloat and release binary size — and is the only tier permitted to update perf/baseline.json. Invoke before committing a performance change, when investigating where time actually goes, when establishing a new reference point, or runs /perf-deep.
---

# Perf deep

The investigation tier for the `dreamd` repo (root = the directory containing this
`.claude/` folder, currently `/Users/oliverfong/toadmountain/dreamd/`). Two jobs, and
they are separate: producing profiles you can actually read, and setting the baseline
every other tier compares against.

This builds debug, release **and** a `profiling` profile, launches the app, and drives
Instruments. Budget twenty minutes and tell the user before starting.

## 1. Check the tools

The tier degrades gracefully — a missing tool is skipped with its install line
printed, not a failure. But a deep run that silently skipped profiling is close to
worthless, so confirm up front:

```sh
command -v hyperfine samply cargo-bloat xctrace
```

Anything missing installs with `brew install hyperfine samply` or
`cargo install cargo-bloat`. Xcode provides `xctrace`. Offer the install rather than
running a hollowed-out tier.

## 2. Run

```sh
./perf/run.sh deep
```

Full criterion sweep, all Chromium scenarios, then the real app **twice** — the debug
workload the pass tier runs (hyperfine at 10 runs, save-loop with 10 highlights) and
the release one (20 runs, **100** highlights) — then `perf/scripts/profile.sh`.

The debug pass is not padding. Real-app metric paths carry the build profile, and only
this tier writes the baseline, so measuring release alone would leave every pass-tier
`real.*` number without a baseline forever — including `events_per_save` and
`save_to_paint_ms`. Deep is a superset of pass, never a substitute for it.

Note the profiling profile exists because `[profile.release]` sets `strip = true`,
which leaves every trace showing raw addresses. The run reports
`profile.symbolicated`. If that is `false`, the traces are unreadable and the profile
half of the tier did not really happen — say so instead of pointing at a useless
`.trace` bundle.

## 3. Read the profiles

Artifacts land in `perf/results/` (gitignored):

- `time-profiler-<sha>.trace` — open with `open perf/results/<name>.trace`.
- `samply-render-<sha>.json.gz` — open with `samply load <path>`.
- `sample-<sha>.txt` — the zero-install fallback, only produced when samply failed.

Read them before drawing conclusions. The point of this tier is to replace a guess
about where time goes with a measurement, so a report that only quotes the summary
table has skipped the actual work.

## 4. Updating the baseline

Only this tier may do it, and only deliberately:

```sh
./perf/run.sh deep --update-baseline
```

Rules, all of which matter:

- **Never** update a baseline to make a regression disappear. If a number got worse
  and you cannot explain why, the baseline is correct and the code is not.
- The new `perf/baseline.json` goes in the **same commit** as the change that
  justified it, with the before/after in the commit message. A baseline moving on its
  own in a separate commit is indistinguishable from a cover-up six months later.
- Update on a quiet machine. A baseline captured next to a running build bakes that
  contention into every future comparison.
- Say what moved and why, in the commit body. "Baseline refresh" is not a reason.

Do not update the baseline without the user's agreement. Ask, showing the deltas.

## 5. Report

Lead with where the time actually goes, from the profiles — that is what this tier is
for. Then the baseline question: whether you believe the deltas are real, and whether
you recommend updating. Name the trace paths so the user can open them. State plainly
which numbers are Chromium-relative and which are the real app.
