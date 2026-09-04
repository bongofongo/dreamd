#!/usr/bin/env bash
#
# The one entry point for dreamd performance measurement.
#
#   ./perf/run.sh quick              ~60-90s   after a meaningful edit
#   ./perf/run.sh pass               ~5min     after a full thread pass
#   ./perf/run.sh deep               ~20min    before a commit / investigating
#   ./perf/run.sh deep --update-baseline
#
# Writes perf/results/<tier>-<sha>-<stamp>.json and prints a table diffing
# against the baseline, if there is one. See "where is the baseline?" below —
# it is not tracked in this repo, and a tree without one still runs every tier.
#
# Baselines are PER MACHINE. Every machine can record and update its own, and a
# comparison only ever happens against a file this machine wrote — those are the
# same rule, since timings are a property of the computer as much as the code.
#
# The baseline is NEVER updated automatically. A baseline that drifts on its own
# hides exactly the slow regression it exists to catch.
#
# Flags:
#   --update-baseline   deep tier only; overwrite the resolved baseline
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

# ---- required tools ------------------------------------------------------
# Everything in perf/README.md's Setup section is optional and is skipped with
# its install line printed. These two are not: `node` drives the corpus, the
# Chromium scenarios and the comparison, and `jq` assembles every result file
# here and in scripts/. Unchecked, a machine without jq ran the entire sweep,
# wrote nothing, and exited 0 — which is indistinguishable from a clean tier.
# Fail before the lock and before anything is measured.
MISSING_TOOLS=()
for tool in node jq; do
  command -v "$tool" >/dev/null 2>&1 || MISSING_TOOLS+=("$tool")
done
if (( ${#MISSING_TOOLS[@]} )); then
  echo "perf/run.sh requires: ${MISSING_TOOLS[*]}" >&2
  echo "  macOS:   brew install ${MISSING_TOOLS[*]}" >&2
  echo "  Arch:    sudo pacman -S ${MISSING_TOOLS[*]}" >&2
  echo "  Debian:  sudo apt install ${MISSING_TOOLS[*]}" >&2
  exit 2
fi

# ---- which machine is this? ----------------------------------------------
# A timing is a fact about a machine as much as about the code, so a comparison
# is only meaningful against numbers *this* machine produced. That rule used to
# be spelled "Darwin only", which was a proxy for "the one Mac the baseline came
# from" — true while there was one machine, and wrong the moment there were two:
# it left every other machine unable to record a reference of its own, so the
# only regression signal off that Mac was a hand-run A/B.
#
# The identity is recorded in the results and the baseline file is named after
# it. Two machines therefore cannot overwrite each other's references, and
# cannot be diffed against each other by accident — the same two guarantees the
# Darwin check was making, without being confined to one computer.
MACHINE_OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
MACHINE_ARCH="$(uname -m)"
# `uname -n` rather than `hostname`: hostname is not installed on a minimal Arch
# box (it is inetutils, not coreutils), and falling back to a literal "unknown"
# would key every such machine to the same file. uname is POSIX and is already
# being run twice above. The first dot-component only — a Mac's name picks up a
# `.local` suffix that comes and goes with the network, and the id has to be the
# same file every day.
MACHINE_HOST="$(uname -n 2>/dev/null || echo unknown)"
MACHINE_HOST="${MACHINE_HOST%%.*}"
[[ -n "$MACHINE_HOST" ]] || MACHINE_HOST="unknown"
# `sysctl` is macOS, `nproc` is Linux. This was a bare `sysctl … || echo 8`, so
# every Linux run silently recorded 8 cores and sized the load warning against a
# core count it had made up.
NCPU="$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 8)"
case "$MACHINE_OS" in
  darwin) MACHINE_CPU="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo "")" ;;
  linux)  MACHINE_CPU="$(sed -n 's/^model name[[:space:]]*: //p' /proc/cpuinfo 2>/dev/null | head -1)" ;;
  *)      MACHINE_CPU="" ;;
esac
[[ -n "$MACHINE_CPU" ]] || MACHINE_CPU="unknown"
# The id becomes a filename, so it is slugged rather than trusted. os+arch alone
# would collide two different Linux boxes; the hostname is what separates them.
MACHINE_ID="$(printf '%s-%s-%s' "$MACHINE_OS" "$MACHINE_ARCH" "$MACHINE_HOST" \
  | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9._-' '-' | sed -E 's/-+/-/g; s/^-//; s/-$//')"

# ---- where is the baseline? ----------------------------------------------
# The reference numbers are not tracked in this repo. They are working material
# — one machine's, moved by hand in the commit that justified it — and they live
# in the private notes repo, cloned at notes/ and gitignored here.
#
# Resolution order, first hit wins:
#   $DREAMD_PERF_BASELINE                     explicit override, for an A/B
#   notes/perf-baselines/<machine>.json       the checked-out private notes
#   perf/baselines/<machine>.json             a purely local one, also gitignored
#
# None of the three is required. A clone without notes/ — which is what a
# contributor has — runs every tier in full and simply has nothing to diff
# against, and that is *not* an error: the tier's own exit status is the only
# thing that can fail this script. Silence about a comparison nobody can make
# beats a red table about a machine nobody has.
BASELINE=""
BASELINE_APPLIES=0
for candidate in \
  "${DREAMD_PERF_BASELINE:-}" \
  "$ROOT/notes/perf-baselines/$MACHINE_ID.json" \
  "$ROOT/perf/baselines/$MACHINE_ID.json"
do
  if [[ -n "$candidate" && -f "$candidate" ]]; then
    # An explicitly named file is compared whatever machine it came from: naming
    # one is a deliberate act, and A/B against an arbitrary file is what the
    # variable is for. The keyed paths carry this machine's id in the name, so
    # matching one is itself the proof that it applies.
    BASELINE="$candidate"; BASELINE_APPLIES=1; break
  fi
done

# The pre-keying baselines: a single file, no machine recorded. There was one
# machine when they were written and it was the arm64 Mac, so that is the only
# thing an unstamped file may be compared against. Elsewhere it still *resolves*
# — so the run can name the file it is declining to use — but it does not apply.
# The first `deep --update-baseline` on any machine writes a keyed file, which
# outranks these, and the legacy file ages out on its own.
if [[ -z "$BASELINE" ]]; then
  for candidate in "$ROOT/notes/perf-baseline.json" "$ROOT/perf/baseline.json"; do
    if [[ -f "$candidate" ]]; then
      BASELINE="$candidate"
      [[ "$MACHINE_OS" == "darwin" ]] && BASELINE_APPLIES=1
      break
    fi
  done
fi

# Where --update-baseline writes. Not the same question: the file may not exist
# yet, and the first `deep --update-baseline` in a fresh tree has to put it
# somewhere. Prefer the notes clone when it is present, so the numbers keep
# moving through that repo's history rather than accumulating untracked here.
# Always a keyed path — this is what makes updating safe on every machine
# rather than on one, since no run can name another machine's file.
if [[ -n "${DREAMD_PERF_BASELINE:-}" ]]; then
  BASELINE_WRITE="$DREAMD_PERF_BASELINE"
elif [[ -d "$ROOT/notes/.git" ]]; then
  BASELINE_WRITE="$ROOT/notes/perf-baselines/$MACHINE_ID.json"
else
  BASELINE_WRITE="$ROOT/perf/baselines/$MACHINE_ID.json"
fi

# A baseline that is not there is a fine state to be in; one named explicitly
# and then missing is a typo, and saying nothing about it would diff against
# whatever happened to be next in the list.
if [[ -n "${DREAMD_PERF_BASELINE:-}" && ! -f "$DREAMD_PERF_BASELINE" ]]; then
  echo "DREAMD_PERF_BASELINE points at a file that does not exist:" >&2
  echo "  $DREAMD_PERF_BASELINE" >&2
  exit 2
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
# Which machine this is was settled above and is recorded in meta.machine. What
# is left is the part that changes minute to minute: how busy it is right now.
echo "machine: $MACHINE_ID ($MACHINE_CPU, $NCPU cores)" >&2
# macOS says "load averages: 1.2 3.4 5.6", Linux "load average: 1.2, 3.4, 5.6".
# Both are covered; anything else falls back to 0 rather than feeding a sentence
# to --argjson, which would abort the assemble step after the whole sweep ran.
LOAD1="$(uptime | sed -E 's/.*load averages?:[[:space:]]*([0-9.]+).*/\1/')"
[[ "$LOAD1" =~ ^[0-9]+(\.[0-9]+)?$ ]] || LOAD1=0
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

# ONE setting for every tier, defined once so it cannot drift into three.
#
# Cutting samples in the quick tier to save time made its numbers systematically
# slower than the deep baseline's for identical code (fewer warmup iterations,
# colder caches), so every quick run reported half a dozen phantom regressions.
# Speed comes from running fewer benchmarks, never from measuring the same
# benchmark more cheaply — otherwise the result is not comparable to the
# baseline it is checked against. The tiers differ in their filter and nothing
# else.
BENCH_ARGS=(--warm-up-time 1 --measurement-time 3)

say "rust benches ($TIER)"
case "$TIER" in
  quick)
    run_bench locate "${BENCH_ARGS[@]}" 'reanchor/today/(1|10)$|locate_single' || true
    run_bench render "${BENCH_ARGS[@]}" 'render/(mixed|code)/(128k|512k)$' || true
    run_bench search "${BENCH_ARGS[@]}" 'keystrokes/500' || true
    ;;
  pass)
    # Everything except the two cases that dominate wall time: 500 highlights
    # (~4s per iteration) and 2MB renders. Both are in the deep tier, and both
    # show up in the "not measured this run" list here — that is expected, not a
    # failure.
    run_bench locate "${BENCH_ARGS[@]}" 'reanchor/[a-z_]+/(1|10|100)$|locate_single' || true
    run_bench render "${BENCH_ARGS[@]}" 'render/[a-z]+/(8k|128k|512k)$|syntect_cold' || true
    run_bench search "${BENCH_ARGS[@]}" || true
    run_bench walk   "${BENCH_ARGS[@]}" || true
    ;;
  deep)
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
  --arg machine_id "$MACHINE_ID" --arg machine_os "$MACHINE_OS" \
  --arg machine_arch "$MACHINE_ARCH" --arg machine_host "$MACHINE_HOST" \
  --arg machine_cpu "$MACHINE_CPU" \
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
     # meta.machine is what makes a result file self-describing: a baseline is
     # just a result that was kept, so a file found later says which computer it
     # came from instead of relying on where it happens to be filed.
     # report.mjs skips everything under meta., so none of this reaches the table.
     meta: {
       tier: $tier, sha: $sha, stamp: $stamp, load1: $load1, ncpu: $ncpu,
       machine: {
         id: $machine_id, os: $machine_os, arch: $machine_arch,
         host: $machine_host, cpu: $machine_cpu
       }
     },
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
if [[ -z "$BASELINE" ]]; then
  # Not an error. The tier ran, the numbers are on disk, there is simply
  # nothing for this machine to compare them to yet.
  echo "" >&2
  echo "no baseline for $MACHINE_ID — recorded, but not compared." >&2
  echo "raw numbers: ${OUT#"$ROOT"/}" >&2
  echo "establish this machine's reference with: ./perf/run.sh deep --update-baseline" >&2
  echo "  -> ${BASELINE_WRITE#"$ROOT"/}" >&2
elif (( BASELINE_APPLIES )); then
  node perf/lib/compare.mjs "$OUT" "$BASELINE" "$VERBOSE" || STATUS=$?
else
  # Reached only by a legacy unstamped baseline on a machine that is not the Mac
  # that wrote it. Naming the file matters: the alternative reads as "you have no
  # baseline", and the reader goes looking for one that is sitting right there.
  echo "" >&2
  echo "not comparing against ${BASELINE#"$ROOT"/}: it carries no machine stamp," >&2
  echo "so it can only be the arm64 Mac's, and this is $MACHINE_ID." >&2
  echo "raw numbers: ${OUT#"$ROOT"/}" >&2
  echo "give this machine its own with: ./perf/run.sh deep --update-baseline" >&2
  echo "  -> ${BASELINE_WRITE#"$ROOT"/}" >&2
fi

if (( UPDATE_BASELINE )); then
  if [[ "$TIER" != "deep" ]]; then
    echo "" >&2
    echo "refusing to update the baseline from a '$TIER' run — use: ./perf/run.sh deep --update-baseline" >&2
    echo "quick and pass run a subset of the benchmarks and skip the real app, so they" >&2
    echo "would write a baseline with holes in it — deep is the only complete sweep." >&2
    exit 2
  fi
  mkdir -p "$(dirname "$BASELINE_WRITE")"
  cp "$OUT" "$BASELINE_WRITE"
  echo "" >&2
  echo "baseline for $MACHINE_ID updated from $OUT" >&2
  echo "  -> ${BASELINE_WRITE#"$ROOT"/}" >&2
  if [[ "$BASELINE_WRITE" == "$ROOT/notes/"* ]]; then
    echo "commit it in notes/, alongside the dreamd change that justified it." >&2
  else
    echo "this path is gitignored — it is yours alone, and no commit is expected." >&2
  fi
fi

exit $STATUS
