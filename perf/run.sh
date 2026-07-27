#!/usr/bin/env bash
#
# The one entry point for dreamd performance measurement.
#
#   ./perf/run.sh quick              ~60-90s   after a meaningful edit
#   ./perf/run.sh pass               ~5min     after a full thread pass
#   ./perf/run.sh deep               ~15min    before a commit / investigating
#   ./perf/run.sh deep --update-baseline
#
# Writes perf/results/<tier>-<sha>-<stamp>.json and prints a table diffing
# against perf/baseline.json.
#
# The baseline is NEVER updated automatically. A baseline that drifts on its own
# hides exactly the slow regression it exists to catch.
#
# Flags:
#   --update-baseline   deep tier only; overwrite perf/baseline.json
#   --verbose           show every metric, not just the ones that moved
#   --no-window         skip anything that opens a window (loop.sh, first paint)

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TIER="${1:-}"
shift || true

UPDATE_BASELINE=0
VERBOSE=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --update-baseline) UPDATE_BASELINE=1; shift ;;
    --verbose) VERBOSE=1; shift ;;
    --no-window) export DREAMD_PERF_NO_WINDOW=1; shift ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
done

case "$TIER" in
  quick|pass|deep) ;;
  *) echo "usage: ./perf/run.sh {quick|pass|deep} [--update-baseline] [--verbose] [--no-window]" >&2; exit 2 ;;
esac

# ---- whose numbers are these? --------------------------------------------
# perf/baseline.json is one machine's numbers — an arm64 Mac, WKWebView, APFS,
# FSEvents. Diffing a Linux run against it does not measure a regression, it
# measures the two machines, and every row would come back red on a tree nobody
# touched. The tiers still RUN on Linux and still write perf/results/ — what is
# withheld is the comparison, because a comparison against the wrong reference
# is worse than none: it is noise wearing a regression's clothes.
#
# Updating it from here is refused outright for the same reason, one step
# harder: that would replace the reference rather than misread it.
BASELINE_APPLIES=1
if [[ "$(uname -s)" != "Darwin" ]]; then
  BASELINE_APPLIES=0
  if (( UPDATE_BASELINE )); then
    echo "refusing to update perf/baseline.json from $(uname -s)." >&2
    echo "the baseline is a macOS machine's numbers; overwriting it here would" >&2
    echo "silently re-zero every macOS comparison against a different computer." >&2
    exit 2
  fi
fi

# ---- exclusivity ---------------------------------------------------------
# One tier at a time, always. Two runs sharing a machine measure each other,
# and a stray `cargo` or corpus regenerate alongside a run silently corrupts it
# — which is exactly how the first several baselines here got poisoned.
# `mkdir` is atomic, so this is race-free without needing flock.
LOCK="$ROOT/perf/.run.lock"
if ! mkdir "$LOCK" 2>/dev/null; then
  echo "another perf run is in progress (lock: ${LOCK#"$ROOT"/})" >&2
  echo "wait for it, or remove the lock if you're sure it's stale." >&2
  exit 3
fi
cleanup_lock() { rmdir "$LOCK" 2>/dev/null || true; }

# ---- machine state -------------------------------------------------------
# Timings are only meaningful relative to the machine that produced them, so
# the state gets recorded alongside them rather than assumed.
NCPU="$(sysctl -n hw.ncpu 2>/dev/null || echo 8)"
LOAD1="$(uptime | sed -E 's/.*load averages?: ([0-9.]+).*/\1/')"
if awk -v l="$LOAD1" -v n="$NCPU" 'BEGIN { exit !(l > n * 0.6) }'; then
  echo "" >&2
  echo "WARNING: load average is $LOAD1 on $NCPU cores — the machine is busy." >&2
  echo "  Real-app timings will be inflated and are not comparable to a quiet run." >&2
  echo "  Recorded in meta.load1 so the numbers carry their own caveat." >&2
  echo "" >&2
fi

RESULTS="$ROOT/perf/results"
mkdir -p "$RESULTS"
SHA="$(git rev-parse --short HEAD 2>/dev/null || echo nogit)"
STAMP="$(date +%Y%m%d-%H%M%S)"
OUT="$RESULTS/$TIER-$SHA-$STAMP.json"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"; cleanup_lock' EXIT

say() { printf '\n=== %s\n' "$*" >&2; }

# ---- 0. corpus -----------------------------------------------------------

say "corpus"
node perf/corpus/gen.mjs >&2

# ---- 1. chromium harness -------------------------------------------------
#
# Runs BEFORE the Rust benches, and the order is load-bearing — do not "tidy"
# it back. The bench sweep saturates every core for one minute in the quick
# tier and roughly ten in the deep tier. Measuring Chromium afterwards means
# measuring it on a thermally loaded machine, by a different amount in each
# tier: the first baseline captured this way made every subsequent quick run
# report phantom 50-80% improvements, which is exactly the margin a real
# regression would hide inside.
#
# Frontend numbers are latency-sensitive and cheap to collect, so they go first,
# from a comparable machine state in every tier.

harness() {
  local scenario="$1"; shift
  echo "  chromium: $scenario $*" >&2
  ( cd perf/harness && node "scenarios/$scenario.mjs" "$@" 2>/dev/null ) || echo 'null'
}

if [[ -d perf/harness/node_modules ]]; then
  say "chromium harness"
  # Render and scroll use IDENTICAL parameters in every tier, on purpose. They
  # are the two scenarios the quick tier depends on, and a metric is only worth
  # having if it can be compared against the baseline — which only the deep tier
  # writes. Running quick at 512k and deep at 2m gave quick nothing to compare
  # against at all. The tiers differ in how much they run, not in how they run it.
  CH_RENDER="$(harness render --size 2m)"
  CH_SCROLL="$(harness scroll --size 2m --wheels 30)"
  case "$TIER" in
    quick)
      CH_HIGHLIGHT='null'
      CH_PALETTE='null'
      ;;
    pass|deep)
      CH_HIGHLIGHT="$(harness highlight --size 512k)"
      CH_PALETTE="$(harness palette --repo 5000)"
      ;;
  esac
else
  echo "perf/harness/node_modules missing — skipping the Chromium scenarios." >&2
  echo "  set it up with:  cd perf/harness && npm run setup" >&2
  CH_RENDER='null'; CH_SCROLL='null'; CH_HIGHLIGHT='null'; CH_PALETTE='null'
fi

# ---- 2. rust micro-benchmarks -------------------------------------------
# Criterion writes its own baselines into target/criterion; we parse the JSON
# estimates it leaves behind rather than scraping stdout.

# Criterion writes into a per-run directory rather than the shared
# target/criterion. Sweeping the shared one would pick up benchmarks this tier
# never ran, plus results for benchmarks that no longer exist — both of which
# quietly poison the baseline comparison.
export CRITERION_HOME="$WORK/criterion"

run_bench() {
  local name="$1"; shift
  echo "  bench: $name" >&2
  cargo bench --bench "$name" -- "$@" >/dev/null 2>&1 || {
    echo "  bench $name FAILED" >&2
    return 1
  }
}

collect_criterion() {
  # <home>/<group>/<id>/new/estimates.json -> { "group/id": mean_ms }
  local base="$CRITERION_HOME"
  [[ -d "$base" ]] || { echo '{}'; return; }
  find "$base" -name estimates.json -path '*/new/*' 2>/dev/null \
    | while read -r f; do
        local id; id="$(dirname "$(dirname "$f")")"
        id="${id#"$base"/}"
        # Criterion reports nanoseconds; ms is the unit everything else uses.
        jq -c --arg id "$id" '{key: $id, value: (.mean.point_estimate / 1e6)}' "$f"
      done \
    | jq -s 'from_entries'
}

say "rust benches ($TIER)"
case "$TIER" in
  quick)
    # SAME criterion settings as the slower tiers — only the filter differs.
    #
    # Cutting samples here to save time made quick's numbers systematically
    # slower than the deep baseline's for identical code (fewer warmup
    # iterations, colder caches), so every quick run reported half a dozen
    # phantom regressions. Speed comes from running fewer benchmarks, never from
    # measuring the same benchmark more cheaply — otherwise the result is not
    # comparable to the baseline it is checked against.
    BENCH_ARGS=(--warm-up-time 1 --measurement-time 3)
    run_bench locate "${BENCH_ARGS[@]}" 'reanchor/today/(1|10)$|locate_single' || true
    run_bench render "${BENCH_ARGS[@]}" 'render/(mixed|code)/(128k|512k)$' || true
    run_bench search "${BENCH_ARGS[@]}" 'keystrokes/500' || true
    ;;
  pass)
    # Everything except the two cases that dominate wall time: 500 highlights
    # (~4s per iteration) and 2MB renders. Both are in the deep tier, and both
    # show up in the "not measured this run" list here — that is expected, not a
    # failure.
    BENCH_ARGS=(--warm-up-time 1 --measurement-time 3)
    run_bench locate "${BENCH_ARGS[@]}" 'reanchor/[a-z_]+/(1|10|100)$|locate_single' || true
    run_bench render "${BENCH_ARGS[@]}" 'render/[a-z]+/(8k|128k|512k)$|syntect_cold' || true
    run_bench search "${BENCH_ARGS[@]}" || true
    run_bench walk   "${BENCH_ARGS[@]}" || true
    ;;
  deep)
    BENCH_ARGS=(--warm-up-time 1 --measurement-time 3)
    run_bench locate "${BENCH_ARGS[@]}" || true
    run_bench render "${BENCH_ARGS[@]}" || true
    run_bench search "${BENCH_ARGS[@]}" || true
    run_bench walk   "${BENCH_ARGS[@]}" || true
    ;;
esac
BENCHES="$(collect_criterion)"

# ---- 3. real app ---------------------------------------------------------

STARTUP='null'; LOOP='null'; PROFILE='null'
STARTUP_DEBUG='null'; LOOP_DEBUG='null'
case "$TIER" in
  quick)
    : # no app launch in the quick tier; it would blow the time budget
    ;;
  pass)
    say "real app"
    STARTUP_DEBUG="$(perf/scripts/startup.sh --runs 10 2>/dev/null || echo 'null')"
    LOOP_DEBUG="$(perf/scripts/loop.sh --highlights 10 --saves 8 2>/dev/null || echo 'null')"
    ;;
  deep)
    # Deep runs BOTH profiles, and the debug pass is not optional padding.
    # Metric paths carry the build profile, and only the deep tier may write the
    # baseline — so if deep measured release alone, every `real.*` number the
    # pass tier produces would be a debug-keyed path with no baseline to compare
    # against, forever. Deep is a superset of pass: same workloads, plus the
    # release ones on top.
    say "real app (debug — the workload pass compares against)"
    STARTUP_DEBUG="$(perf/scripts/startup.sh --runs 10 2>/dev/null || echo 'null')"
    LOOP_DEBUG="$(perf/scripts/loop.sh --highlights 10 --saves 8 2>/dev/null || echo 'null')"
    say "real app (release)"
    STARTUP="$(perf/scripts/startup.sh --release --runs 20 2>/dev/null || echo 'null')"
    LOOP="$(perf/scripts/loop.sh --release --highlights 100 --saves 12 2>/dev/null || echo 'null')"
    say "profiling"
    PROFILE="$(perf/scripts/profile.sh 2>/dev/null || echo 'null')"
    ;;
esac

# ---- 4. assemble ---------------------------------------------------------

jq -n \
  --arg tier "$TIER" --arg sha "$SHA" --arg stamp "$STAMP" \
  --argjson load1 "${LOAD1:-0}" --argjson ncpu "$NCPU" \
  --argjson benches "$BENCHES" \
  --argjson startup "$STARTUP" --argjson loop "$LOOP" --argjson profile "$PROFILE" \
  --argjson startup_debug "$STARTUP_DEBUG" --argjson loop_debug "$LOOP_DEBUG" \
  --argjson render "$CH_RENDER" --argjson scroll "$CH_SCROLL" \
  --argjson highlight "$CH_HIGHLIGHT" --argjson palette "$CH_PALETTE" \
  '
   # Scenario results are keyed by the parameters that produced them. The tiers
   # deliberately run different workloads — quick renders 512k, deep renders 2m;
   # pass seeds 10 highlights, deep seeds 100 — and writing those to a shared
   # metric path silently compared a 512k document against a 2m one. Keying by
   # workload means non-matching runs land on different paths and show up as
   # "not measured this run" instead of as a fake 80% improvement.
   def keyed(r; k): if r == null or r.skipped then null else {(k): r} end;
   # Deep produces two entries per real-app metric (debug + release); pass
   # produces one. Merging keeps them side by side under distinct keys instead
   # of one overwriting the other.
   def both(a; b): ((a // {}) + (b // {})) | if . == {} then null else . end;
   {
     meta: { tier: $tier, sha: $sha, stamp: $stamp, load1: $load1, ncpu: $ncpu },
     bench: $benches,
     # Keyed by build profile as well as workload. `pass` measures the debug
     # binary and `deep` measures both — the same code differs several-fold
     # between profiles, so sharing a metric path reported the profile difference
     # as a 250% regression. Debug and release numbers now simply never meet;
     # deep records both so each has a baseline of its own kind.
     real: {
       startup: both(keyed($startup_debug; $startup_debug.profile // "unknown");
                     keyed($startup;       $startup.profile       // "unknown")),
       loop:    both(keyed($loop_debug; "\($loop_debug.profile // "unknown")-h\($loop_debug.highlights // 0)");
                     keyed($loop;       "\($loop.profile       // "unknown")-h\($loop.highlights       // 0)"))
     },
     chromium: {
       render:    keyed($render;    "\($render.variant)-\($render.size)"),
       scroll:    keyed($scroll;    "\($scroll.variant)-\($scroll.size)-hl\($scroll.highlights)-w\($scroll.wheels // 0)"),
       highlight: keyed($highlight; "\($highlight.variant)-\($highlight.size)"),
       palette:   keyed($palette;   "repo\($palette.repoFiles)")
     },
     profile: $profile
   }' >"$OUT"

say "results -> ${OUT#"$ROOT"/}"

# ---- 5. compare ----------------------------------------------------------

# `|| STATUS=$?` rather than a bare call: compare.mjs exits 1 when something
# regressed, and under `set -e` that aborted the script right here — silently
# skipping the `--update-baseline` step below and making the flag a no-op
# exactly when a run had findings.
STATUS=0
if (( BASELINE_APPLIES )); then
  node perf/lib/compare.mjs "$OUT" "$ROOT/perf/baseline.json" "$VERBOSE" || STATUS=$?
else
  echo "" >&2
  echo "no baseline comparison on $(uname -s) — perf/baseline.json is macOS-only." >&2
  echo "raw numbers: ${OUT#"$ROOT"/}" >&2
  echo "to detect a regression here, A/B this tree against the one you changed it from" >&2
  echo "on this same machine — that is what .github/workflows/perf.yml does in CI." >&2
fi

if (( UPDATE_BASELINE )); then
  if [[ "$TIER" != "deep" ]]; then
    echo "" >&2
    echo "refusing to update the baseline from a '$TIER' run — use: ./perf/run.sh deep --update-baseline" >&2
    echo "the quick and pass tiers use reduced sample counts, so their numbers are not a reference point." >&2
    exit 2
  fi
  cp "$OUT" "$ROOT/perf/baseline.json"
  echo "" >&2
  echo "baseline updated from $OUT" >&2
  echo "commit it alongside the change that justified it." >&2
fi

exit $STATUS
