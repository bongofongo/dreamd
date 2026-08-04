# dreamd performance harness

Local performance measurement. No CI, no GitHub Actions — this exists to be run by
hand and by Claude, on this machine, during a session.

```sh
./perf/run.sh quick     # ~60s    
./perf/run.sh pass      # ~5min   
./perf/run.sh deep      # ~20min  
```

Results go to `perf/results/` (gitignored). Each run prints a diff against the
baseline — **if this checkout has one.** The baseline is not tracked here; see
[Baselines](#baselines). Without it every tier still runs in full and simply
records instead of comparing, which is what a fresh clone does.

## Setup

Nothing is required for the Rust benches. The rest:

```sh
brew install hyperfine samply      # startup timing, flamegraphs
cargo install cargo-bloat          # binary composition (deep tier)
cd perf/harness && npm run setup   # Playwright + Chromium (test-only)
```

Every tool is optional. A missing one is skipped with its install line printed rather
than failing the run — but a tier that skipped half its work says so, and you should
believe it.

**Nothing here ships.** `perf/harness/` has its own `package.json`, `node_modules` is
gitignored, and `tauri.conf.json` still points `frontendDist` at `../ui`. Node is a
test dependency and never enters the binary.

## What each layer measures, and how much to trust it

| Layer | Source | Trust |
|---|---|---|
| `bench.*` | criterion, native Rust | Real. Statistical. Quote freely. |
| `real.*` | the actual binary, WKWebView, NDJSON timing marks | Real. The numbers that matter. |
| `chromium.*` | Playwright Chromium + stubbed `window.__TAURI__` | **Relative only.** Chromium is not WKWebView. |

One measurement sits outside that table. `release_binary_bytes` (deep tier,
`scripts/profile.sh`) measures `cargo build --release`, which is **not** the
binary that ships: `cargo tauri build` adds `--features tauri/custom-protocol`,
which flips tauri's `dev` flag and changes what gets embedded. Treat it as a
relative signal for "did the binary grow"; measure the shipped size with
`packaging/build.sh` when the actual number matters.

The Chromium layer exists because driving a real WKWebView is slow and flaky, and
because the quick tier has to finish in ninety seconds. It reliably tells you *that*
the frontend got slower. It does not tell you what the app's frame time is. Every
report should keep that distinction visible.

## Layout

```
perf/
├── run.sh                  the only entry point
├── baseline.json           reference numbers, if any — gitignored, see Baselines
├── corpus/gen.mjs          deterministic fixture generator
├── corpus/manifest.json    committed sizes + sha256
├── lib/report.mjs          flatten + diff + render the table
├── scripts/startup.sh      hyperfine + phase marks, cold start
├── scripts/loop.sh         save -> repaint, the core product loop
├── scripts/profile.sh      Instruments / samply / cargo-bloat
└── harness/                Playwright scenarios + the __TAURI__ stub
```

## The corpus

`node perf/corpus/gen.mjs` builds ~21MB of deterministic fixtures: four document
variants (`prose`, `code`, `table`, `mixed`) at 8KB / 128KB / 512KB / 2MB, synthetic
repos of 10 / 500 / 5000 markdown files, and highlight sets of 1 / 10 / 100 / 500.
`--stress` adds 8MB documents.

It is seeded and byte-identical across runs, which is the whole point: a benchmark
number is only comparable if its input is. `manifest.json` records every file's
sha256, and regeneration is skipped when the manifest still matches disk. Bump
`GENERATOR` in `gen.mjs` when a change to the generator invalidates recorded numbers.

Highlight fixtures carry both `quote` (the exact source slice) and `rendered` (the
same text with whitespace collapsed). The distinction is load-bearing: the app stores
what `getSelection().toString()` returns, which comes from the rendered DOM and never
matches raw source. Benchmarking with `quote` measures a path the app never takes and
understates the real cost by roughly 10x.

## Instrumentation

The real-app numbers come from a `perf` cargo feature that is off by default:

```sh
cargo build --features perf
```

It emits one NDJSON record per mark on **stderr** — stderr because `console.log`
inside WKWebView never reaches the process's stdout:

```json
{"dreamd_perf":1,"phase":"walk_done","ms":12.418}
```

Rust marks carry ms since process start. Frontend marks come back through the
`perf_mark` command; phases prefixed `d:` are durations rather than timestamps.
`--bench-startup` runs the pre-window sequence and exits, and `DREAMD_PERF_SEED`
preloads highlights from a corpus fixture so the save loop can be measured at a
realistic highlight count without driving the UI.

## Noise, and why the thresholds differ

Every measurement here has a floor below which a "change" is just the machine.
These were measured, not guessed — two consecutive runs on identical code:

| source | run-to-run drift | warn / fail |
|---|---|---|
| Rust benches | ~5-12%, mixed sign | 5% / 15% |
| Chromium scenarios | up to 27% (raster, composite) | 20% / 35% |
| any `.max` statistic | up to 25% | ignored entirely |

A threshold set below the noise floor doesn't catch more regressions — it just
trains you to ignore the tool.

**Every tier uses identical criterion settings.** The quick tier is fast because
it runs *fewer* benchmarks, never because it measures the same benchmark more
cheaply. An earlier version cut `--sample-size` to 10 for speed, which made quick's
numbers systematically slower than the deep baseline's for identical code — fewer
warmup iterations, colder caches — and every quick run reported half a dozen
phantom regressions. Same rule for the Chromium scenarios: render and scroll take
the same document and wheel count in all three tiers.

The general rule, learned the hard way several times here: **if two runs are to be
compared, everything about the workload must match — size, counts, sample settings.
Anything that differs belongs in the metric's key, not silently in its value.**

Three things keep the real-app numbers honest, since those cannot be sampled
statistically the way a microbenchmark can:

- **A lock.** One tier at a time. A second run, or a stray `cargo` alongside one,
  is refused rather than silently averaged in.
- **Load recorded in `meta`.** A timing with no record of the machine's state
  can't be audited later, and a warning prints above 60% of cores.
- **Launches take the minimum of 3**, not the mean. Contention is strictly
  additive — it can only make a launch slower — so the fastest run is the closest
  thing to an uncontended one. This took first-paint spread from roughly 3x
  (1.5s-5.2s across single runs) down to under 60ms, and is what makes the number
  a property of the code rather than of the moment it was taken. `spread_ms` is
  reported alongside; a large spread means distrust even the minimum.

## Baselines

**The baseline is not in this repo.** It is one developer's machine — an arm64
Mac, WKWebView, APFS, FSEvents — and it is working material rather than product,
so it lives in the private notes repo alongside the session log. `run.sh`
resolves it, first hit wins:

| | |
| --- | --- |
| `$DREAMD_PERF_BASELINE` | explicit override, for an A/B against an arbitrary file |
| `notes/perf-baseline.json` | the private notes repo, cloned at `notes/` |
| `perf/baseline.json` | a purely local one — gitignored, never committed here |

None of the three is required, and a missing baseline is **not an error**: the
tier runs, `perf/results/` is written, the comparison is skipped with a line
saying so, and the exit status is the tier's own. A red table about a machine
nobody has would be worse than no table.

The baseline is only ever written by `./perf/run.sh deep --update-baseline`, to
whichever of the three paths resolves for writing (the notes clone when it is
there, `perf/baseline.json` otherwise). Never by `quick` or `pass` — their
reduced sample counts are not a reference point, and the runner refuses. It is
also refused off Darwin, where it would re-zero every macOS comparison against a
different computer.

A baseline that drifts on its own hides exactly the slow regression it exists to
catch. Update it deliberately, in the same commit as the change that justified it,
with the before/after in the commit message — that commit lands in the notes repo,
next to the dreamd commit it belongs to.

**The deep tier runs the real app twice**, once debug and once release, and that is
what makes the pass tier's `real.*` numbers checkable at all. Metric paths carry the
build profile (`real.loop.debug-h10` vs `real.loop.release-h100`) so debug and release
figures never meet, and only `deep` may write the baseline — so a deep tier that
measured release alone would leave every pass-tier `real.*` metric permanently
baseline-less, including `events_per_save` and `save_to_paint_ms`. Deep is a superset
of pass: the same debug workload pass runs, plus the release one on top.

**Known-stale:** the current baseline's `real.*` entries predate profile keying and
sit at the old paths, so every `real.*` metric shows as `new` and the old names show
under "not measured this run". The next `./perf/run.sh deep --update-baseline`
realigns them — for both profiles, now that deep measures both. `bench.*` and
`chromium.*` are current. The baseline is deliberately not hand-edited to patch this:
a baseline you can edit by hand is not evidence of anything.

## Gotchas worth knowing

Three things bit this harness during construction, all of which would silently
produce plausible-looking but wrong numbers:

- **`grep -q` under `set -o pipefail`.** `grep -q` exits on first match, the upstream
  command dies of SIGPIPE, and the pipeline reports failure despite the match
  succeeding. This reported every symbolicated binary as stripped. Use `grep -c`
  in a condition.
- **Quotes must come from the right side.** `locate` matches against markdown
  *source*; `applyHighlights` matches against the rendered *DOM*. A fixture built for
  one measures nothing useful in the other — source-derived quotes still carry
  markdown syntax and fail 100% of DOM matches.
- **`resolve_target` roots the tree at the current directory** when handed a file
  argument, and the watcher follows the root. `loop.sh` therefore launches from
  inside its scratch repo; launching from the dreamd repo watches the wrong tree and
  silently records zero events.

- **Measurement order affects the measurement.** The Chromium scenarios run before
  the Rust benches, because the bench sweep pins every core for one to ten minutes
  depending on tier, and frontend numbers collected afterwards are thermally
  inflated by a tier-dependent amount. The first baseline built the other way round
  made every quick run report phantom 50-80% improvements.

Also: don't run `gen.mjs --force` while a tier is running. The runner regenerates the
corpus itself at the start of every run; a concurrent regenerate deletes files out
from under a bench mid-read.

## Adding a metric

Emit a number anywhere in any script's JSON output. `lib/report.mjs` flattens results
to dot-paths and diffs everything numeric it finds, so a new measurement appears in
the table with no registry to update. Add its path to `IGNORE` there if it is
descriptive rather than a performance figure.
