# dreamd performance harness

Local performance measurement — this exists to be run by hand and by Claude, on
this machine, during a session. Nothing here gates a build. The one automated
caller is `release.yml`'s `perf-history` job, which runs the deep tier on a
published release to capture four public numbers for the website chart (see
`scripts/publish-history.sh`); it is `continue-on-error` and compares against no
baseline, since a hosted runner is nobody's machine.

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

**Two tools are required**, and `run.sh` refuses to start without them rather than
measuring for a minute and writing nothing:

```sh
brew install jq          # macOS
sudo pacman -S jq        # Arch
sudo apt install jq      # Debian / Ubuntu
```

`node` is the other, and is assumed — it drives the corpus generator, the Chromium
scenarios and the comparison. `jq` assembles every result file, here and in
`scripts/`. A machine without it used to run the whole sweep, produce no result
file, and exit 0, which reads exactly like a tier that passed; now it is a
two-line failure before anything is measured.

Everything else is optional and is skipped with its install line printed:

```sh
brew install hyperfine samply       # startup timing, flamegraphs (macOS)
cargo install hyperfine samply      # the same two, any platform
cargo install cargo-bloat           # binary composition (deep tier)
cd perf/harness && npm run setup    # Playwright + Chromium (test-only)
```

A tier that skipped half its work says so, and you should believe it.

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
├── baselines/<machine>.json this machine's reference numbers — gitignored, see Baselines
├── corpus/gen.mjs          deterministic fixture generator
├── corpus/manifest.json    committed sizes + sha256
├── lib/report.mjs          flatten + diff + render the table
├── lib/compare.mjs         the CLI wrapper run.sh calls; exit 1 on a regression
├── scripts/startup.sh      hyperfine + phase marks, cold start
├── scripts/loop.sh         save -> repaint, the core product loop
├── scripts/profile.sh      Instruments / samply / cargo-bloat
└── harness/                Playwright scenarios + the __TAURI__ stub
```

## The corpus

`node perf/corpus/gen.mjs` builds ~24MB of deterministic fixtures: five document
variants (`prose`, `code`, `table`, `mixed`, `images`) at 8KB / 128KB / 512KB /
2MB, synthetic repos of 10 / 500 / 5000 markdown files, and highlight sets of
1 / 10 / 100 / 500. `--stress` adds 8MB documents.

`images` is prose carrying image references at about the density of a
documentation page, against four small PNGs in `docs/img/` that the generator
encodes itself. It is **last** in the variant list on both sides — `gen.mjs` and
`benches/common.rs` — because documents are seeded in that order, so anything
inserted above it would renumber the sixteen documents every recorded number was
measured against. Its own rows are new and the baseline has none until a deep
tier records them.

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
| `real.loop.*` frontend awaits | ~9% | 15% / 30% |
| `real.loop.*.rust_*` | ~1% | 5% / 15% |
| any `.max` statistic | up to 25% | ignored entirely |
| any `real.*` timing under 1ms | — | never flagged |

**The save loop's frontend metrics were bimodal until block patching landed**,
and the history is worth keeping because the threshold moved twice. Five
`loop.sh --release --highlights 100 --saves 12` runs on *identical* code once
gave p50s of `save_to_paint` 1669-3369ms and `ipc_render_markdown` 177-1307ms,
while `rust_reanchor_ms.p50` sat between 62.31 and 62.92ms across the same five.

Each of those metrics times an `await`, and whichever await yielded first also
absorbed the webview re-laying out the whole document (see the first gotcha
below), so the cost teleported between metrics from run to run. `writeContent`
now replaces only the blocks whose HTML changed, which removed that layout: the
same five runs give `save_to_paint` 382-390ms, `ipc_render_markdown` 111-114ms,
`apply_highlights` 60-63ms, `ipc_reanchor` 64-70ms. The widest is ±9%, so the
threshold came back down to 15%/30% — a 100% threshold left in place after the
cause was fixed would hide exactly the regression it was standing in for.

`rust_reanchor_ms` remains the steadiest number on this path at ~1%, and
`real.startup.*` is stabilised by min-of-3 and reports its own `spread_ms`
alongside — **read that spread before reading anything else in a run**. It is
how a contended machine announces itself: one `pass` here came back with
`spread_ms` at 131 against a usual 4, and every real-app row in it was fiction.

**Sub-millisecond timings are never flagged.** Every `real.*` leaf is in ms and
several are microseconds — `d:rust_get_highlights` is 0.0015ms, `process_start`
0.002ms — so a 5% threshold flags them constantly: `0.00150 -> 0.00180` was
printed as a 20% regression, which is 300 nanoseconds of scheduler jitter. A row
whose baseline *and* current value are both under 1ms keeps its numbers in the
table but takes no status. `events_per_save` is exempt, being a ratio around 1.0
rather than a duration: a move from 1 to 2 means every save renders the document
twice, which is the most valuable thing the loop can report.

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

**`first_paint` is "boot work done", not "pixels on glass" — and its meaning
narrowed on 2026-09-04.** The mark fires when the frontend's init sequence
(IPC, DOM write, decoration, highlights) completes. It used to *also* carry the
webview's full layout of the document, because a scroll restore forced that
layout synchronously inside the sequence; the restore is now guarded off the
boot path, so layout happens at frame time after the mark. Numbers straddling
that commit are not comparable (release fell 1266ms → ~330ms with pixels
arriving well after), and the perf-history chart the release job feeds crossed
the same step. What the mark still measures — everything on the critical path
that dreamd's own code controls — it measures more purely than before.

## Baselines

**Baselines are per machine, and none of them is in this repo.** A timing is a
fact about a computer as much as about the code, so there is one reference file
per machine and a comparison only ever happens against a file *this* machine
wrote. They are working material rather than product, so they live in the
private notes repo alongside the session log when it is cloned.

The machine's identity is `<os>-<arch>-<hostname>` — `darwin-arm64-mbp`,
`linux-x86_64-devbox` — computed from `uname`, slugged because it becomes a
filename, and recorded in every result file under `meta.machine`. A result file
therefore says which computer produced it, which matters because a baseline is
just a result that was kept. `run.sh` prints the id at the top of every run.

`run.sh` resolves the baseline, first hit wins:

| | |
| --- | --- |
| `$DREAMD_PERF_BASELINE` | explicit override, for an A/B against an arbitrary file |
| `notes/perf-baselines/<machine>.json` | the private notes repo, cloned at `notes/` |
| `perf/baselines/<machine>.json` | a purely local one — gitignored, never committed here |

None of the three is required, and a missing baseline is **not an error**: the
tier runs, `perf/results/` is written, the comparison is skipped with a line
saying so, and the exit status is the tier's own. A red table about a machine
nobody has would be worse than no table.

An explicitly named `$DREAMD_PERF_BASELINE` is compared whatever machine it came
from — naming one is a deliberate act, and an A/B against an arbitrary file is
what the variable is for. The keyed paths need no such check: matching one means
this machine's id is in the filename.

The baseline is only ever written by `./perf/run.sh deep --update-baseline`, and
always to a keyed path (the notes clone when it is there, `perf/baselines/`
otherwise). Never by `quick` or `pass` — they run a subset of the benchmarks and
skip the real app, so a baseline written from one would have holes in it rather
than numbers, and the runner refuses. (Not because they sample more cheaply:
every tier uses identical criterion settings, as above.) **Every machine can update its own**,
which is safe precisely because the filename carries the id: no run can name
another machine's file, so the guarantee the old Darwin-only rule was making —
that a Linux run cannot re-zero a macOS reference — now holds without confining
the tool to one computer.

A baseline that drifts on its own hides exactly the slow regression it exists to
catch. Update it deliberately, in the same commit as the change that justified it,
with the before/after in the commit message — that commit lands in the notes repo,
next to the dreamd commit it belongs to.

### The pre-keying baselines

`notes/perf-baseline.json` and `perf/baseline.json` are the old single-file
names, written before baselines were keyed. There was one machine then and it
was the arm64 Mac, so an unstamped file is compared **only on macOS**. Elsewhere
it still resolves, so the run can name the file it is declining to use rather
than claiming there is no baseline at all while one sits right there. The first
`deep --update-baseline` on any machine writes a keyed file, which outranks
these, and the old name ages out on its own. Nothing needs migrating by hand.

**The deep tier runs the real app twice**, once debug and once release, and that is
what makes the pass tier's `real.*` numbers checkable at all. Metric paths carry the
build profile (`real.loop.debug-h10` vs `real.loop.release-h100`) so debug and release
figures never meet, and only `deep` may write the baseline — so a deep tier that
measured release alone would leave every pass-tier `real.*` metric permanently
baseline-less, including `events_per_save` and `save_to_paint_ms`. Deep is a superset
of pass: the same debug workload pass runs, plus the release one on top.

**Known-stale:** the arm64 Mac's baseline has `real.*` entries that predate profile
keying and sit at the old paths, so every `real.*` metric shows as `new` and the old
names show under "not measured this run". The next `./perf/run.sh deep
--update-baseline` on that machine realigns them — for both profiles, now that deep measures both. `bench.*` and
`chromium.*` are current. The baseline is deliberately not hand-edited to patch this:
a baseline you can edit by hand is not evidence of anything.

## Gotchas worth knowing

Five things bit this harness during construction, all of which would silently
produce plausible-looking but wrong numbers:

- **A frontend `d:ipc_*` span is not the cost of that IPC call.** It is the wall
  time of an `await`, and the first `await` after `contentEl.innerHTML = html`
  also contains the webview laying out the entire document. On the 2MB corpus
  doc that is close to a second, and it lands on whichever IPC call happens to
  come first — `d:ipc_get_highlights` at boot, `d:ipc_reanchor` in the save
  loop. Measured: `d:ipc_get_highlights` reported **1254ms** while the Rust body,
  timed by `perf::span`, took **0.0015ms**; `d:ipc_reanchor` reported 1020ms
  against a `reanchor_today/100` bench of 64.5ms. The same work also read 276ms
  in one run and 1248ms in another, purely on where the yield fell. Read the
  `d:rust_*` mark beside it before concluding anything about Rust, and do not
  "fix" the span by forcing layout earlier — tried, and it made first paint
  *worse* (1394ms -> 1851ms), because the forced layout no longer overlaps the
  IPC round trip.

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
