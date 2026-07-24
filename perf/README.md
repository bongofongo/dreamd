# dreamd performance harness

Local performance measurement. No CI, no GitHub Actions — this exists to be run by
hand and by Claude, on this machine, during a session.

```sh
./perf/run.sh quick     # ~60s    after an edit
./perf/run.sh pass      # ~5min   after a thread pass
./perf/run.sh deep      # ~15min  before a commit, or when investigating
```

Results go to `perf/results/` (gitignored). Each run prints a diff against
`perf/baseline.json`, which is committed.

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

The Chromium layer exists because driving a real WKWebView is slow and flaky, and
because the quick tier has to finish in ninety seconds. It reliably tells you *that*
the frontend got slower. It does not tell you what the app's frame time is. Every
report should keep that distinction visible.

## Layout

```
perf/
├── run.sh                  the only entry point
├── baseline.json           committed reference numbers
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

`perf/baseline.json` is only ever written by `./perf/run.sh deep --update-baseline`.
Never by `quick` or `pass` — their reduced sample counts are not a reference point,
and the runner refuses.

A baseline that drifts on its own hides exactly the slow regression it exists to
catch. Update it deliberately, in the same commit as the change that justified it,
with the before/after in the commit message.

**Known-stale:** the committed baseline's `real.*` entries were captured before
real-app metrics were keyed by build profile, so they sit at the old paths and will
show as `new` until the next `./perf/run.sh deep --update-baseline`. `bench.*` and
`chromium.*` are current. The baseline was deliberately not hand-edited to patch
this — a baseline you can edit by hand is not evidence of anything.

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
