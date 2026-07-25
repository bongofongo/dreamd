#!/usr/bin/env bash
#
# Cold-start timing against the real binary.
#
#   perf/scripts/startup.sh [--release] [--runs N]
#
# Two layers, because they answer different questions:
#
#   1. `--bench-startup` — the pre-window Rust sequence (resolve target, load
#      config, walk the repo, build the search index) then exit. Measured with
#      hyperfine, so it includes process spawn and dyld. This is the half we
#      control directly. Run once per repo size, plus once on a *file*
#      argument, where the walk is deferred to a background thread and so
#      should not appear in the number at all.
#
#   2. Full launch to `first_paint` — the real app, real WKWebView, parsed out
#      of the NDJSON perf stream. This is what the user actually waits for.
#      Requires a window, so it's one run per invocation rather than a
#      hyperfine sweep.
#
# Emits JSON on stdout. Everything human-readable goes to stderr so the runner
# can capture one and show the other.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

PROFILE_ARGS=()
BIN="target/debug/dreamd"
PROFILE="debug"
RUNS=20
while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) PROFILE_ARGS=(--release); BIN="target/release/dreamd"; PROFILE="release"; shift ;;
    --runs) RUNS="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

# Absolute, because `first_paint_run` cds into a scratch repo before launching:
# a relative path silently fails to resolve there, and the measurement comes
# back empty rather than wrong, which is easy to miss.
BIN="$ROOT/$BIN"

CORPUS="$ROOT/perf/corpus/generated"
SMALL="$CORPUS/repos/repo-10"
LARGE="$CORPUS/repos/repo-5000"

if [[ ! -d "$LARGE" ]]; then
  echo "corpus missing — run: node perf/corpus/gen.mjs" >&2
  exit 1
fi

# Any document inside the large repo, picked deterministically. `dreamd
# file.md` defers the walk entirely, so this case measures the pre-window path
# that a single-file launch actually pays for — without it the hyperfine sweep
# only ever sees the directory path and the win is invisible here.
# No `| head -1`: it closes the pipe early, `sort` dies on SIGPIPE, and under
# `set -o pipefail` that takes the whole script with it.
LARGE_DOC="$(find "$LARGE" -name '*.md' | sort)"
LARGE_DOC="${LARGE_DOC%%$'\n'*}"

echo "building (--features perf ${PROFILE_ARGS[*]:-})..." >&2
cargo build --features perf "${PROFILE_ARGS[@]}" >&2

# ---- layer 1: pre-window sequence, via hyperfine -------------------------

hyperfine_json="$(mktemp)"
trap 'rm -f "$hyperfine_json"' EXIT

if command -v hyperfine >/dev/null 2>&1; then
  hyperfine \
    --warmup 3 --runs "$RUNS" \
    --export-json "$hyperfine_json" \
    --command-name "prewindow/repo-10" "$BIN --bench-startup $SMALL" \
    --command-name "prewindow/repo-5000" "$BIN --bench-startup $LARGE" \
    --command-name "prewindow/single-file" "cd $LARGE && $BIN --bench-startup $LARGE_DOC" \
    >&2
  prewindow="$(jq '[.results[] | {name: .command, mean_ms: (.mean*1000), stddev_ms: (.stddev*1000), min_ms: (.min*1000)}]' "$hyperfine_json")"
else
  echo "hyperfine not installed — skipping the statistical sweep." >&2
  echo "  install it with:  brew install hyperfine" >&2
  prewindow='[]'
fi

# ---- layer 1b: phase breakdown from the NDJSON stream --------------------
# hyperfine gives totals; the marks give the split inside them.

phases_for() {
  "$BIN" --bench-startup "$1" 2>&1 >/dev/null \
    | grep '"dreamd_perf"' \
    | jq -s '[.[] | {(.phase): .ms}] | add'
}

phases_small="$(phases_for "$SMALL")"
phases_large="$(phases_for "$LARGE")"

# ---- layer 2: full launch to first paint ---------------------------------
# The app needs a document to open, and needs to be told to quit once painted.
# We launch it on the 2MB mixed corpus doc, wait for `first_paint` to appear on
# the stream, then kill it. If it never appears, the timeout is itself the
# finding — report it rather than hanging the tier.

first_paint_run() {
  local doc="$1"
  local cwd="$2"
  local out; out="$(mktemp)"
  # Launched from `cwd` because `resolve_target` roots the tree at the repo of
  # the *current directory* when the argument is a file. Running this from the
  # dreamd repo would measure a 7-file tree walk and understate cold start on
  # anything real.
  #
  # `exec` matters: without it `$!` is the subshell's pid, the kill below hits
  # the subshell instead of the app, and every run leaks a live dreamd process.
  ( cd "$cwd" && exec "$BIN" "$doc" >/dev/null 2>"$out" ) &
  local pid=$!
  local waited=0
  while (( waited < 200 )); do
    if grep -q '"first_paint"' "$out" 2>/dev/null; then break; fi
    if ! kill -0 "$pid" 2>/dev/null; then break; fi
    sleep 0.1; waited=$((waited + 1))
  done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  # An empty result means the app never reported a first paint — it crashed, was
  # killed from outside, or timed out. Say so: silently emitting `{}` turns a
  # failed measurement into a missing metric nobody looks at.
  if ! grep -q '"first_paint"' "$out" 2>/dev/null; then
    echo "  WARNING: no first_paint from $doc — measurement lost" >&2
  fi
  grep '"dreamd_perf"' "$out" | jq -s '[.[] | {(.phase): .ms}] | add // {}'
  rm -f "$out"
}

# Repeat the launch and keep the FASTEST run, not the average.
#
# Contention is strictly additive: another process can only ever make a launch
# slower, never faster. So the minimum across N runs is the closest thing to an
# uncontended measurement, and it is far more stable than a mean or median —
# single launches of the same binary measured anywhere from 1.5s to 5.2s here,
# purely on what else the machine happened to be doing.
#
# This is what makes the number a property of the code rather than of the moment
# it was taken. `spread_ms` is reported alongside: a large spread means the
# machine was noisy and even the minimum deserves suspicion.
best_of() {
  local n="$1" doc="$2" cwd="$3"
  local runs="[]" r
  for _ in $(seq 1 "$n"); do
    r="$(first_paint_run "$doc" "$cwd")"
    runs="$(jq -c --argjson r "$r" '. + [$r]' <<<"$runs")"
  done
  jq -c '
    map(select(.first_paint != null)) as $ok
    | if ($ok | length) == 0 then {}
      else ($ok | min_by(.first_paint))
           + { samples: ($ok | length),
               spread_ms: (($ok | max_by(.first_paint).first_paint)
                           - ($ok | min_by(.first_paint).first_paint)) }
      end' <<<"$runs"
}

LAUNCH_RUNS="${DREAMD_PERF_LAUNCH_RUNS:-3}"

if [[ "${DREAMD_PERF_NO_WINDOW:-0}" == "1" ]]; then
  echo "DREAMD_PERF_NO_WINDOW=1 — skipping the windowed launch." >&2
  launch='{}'
  launch_small='{}'
else
  # A scratch clone of the 5000-file repo with the 2MB document dropped inside,
  # so first paint is measured against a realistic tree rather than a toy one.
  # The corpus itself is left alone — adding a file there would change its digest
  # and trigger a full regenerate on the next run.
  SCRATCH="$(mktemp -d)"
  trap 'rm -f "$hyperfine_json"; rm -rf "$SCRATCH"' EXIT
  # -c uses APFS clonefile: copying 10MB across 5000 files is otherwise slow
  # enough to notice on every run.
  cp -Rc "$LARGE" "$SCRATCH/repo" 2>/dev/null || cp -R "$LARGE" "$SCRATCH/repo"
  cp "$CORPUS/docs/mixed-2m.md" "$SCRATCH/repo/doc.md"

  echo "launching for first-paint measurement x$LAUNCH_RUNS, 5000-file repo (windows will flash)..." >&2
  launch="$(best_of "$LAUNCH_RUNS" "$SCRATCH/repo/doc.md" "$SCRATCH/repo")"

  # The same document in a trivial repo. The difference between the two isolates
  # what the tree walk actually costs at startup, as opposed to the render.
  mkdir -p "$SCRATCH/small/.git"
  printf 'ref: refs/heads/main\n' >"$SCRATCH/small/.git/HEAD"
  cp "$CORPUS/docs/mixed-2m.md" "$SCRATCH/small/doc.md"
  echo "launching for first-paint measurement x$LAUNCH_RUNS, small repo..." >&2
  launch_small="$(best_of "$LAUNCH_RUNS" "$SCRATCH/small/doc.md" "$SCRATCH/small")"
fi

# ---- peak memory ---------------------------------------------------------
# macOS /usr/bin/time -l reports "maximum resident set size" in bytes.

rss_bytes="$(/usr/bin/time -l "$BIN" --bench-startup "$LARGE" 2>&1 >/dev/null \
  | awk '/maximum resident set size/ {print $1}')"

jq -n \
  --argjson prewindow "$prewindow" \
  --argjson phases_small "${phases_small:-null}" \
  --argjson phases_large "${phases_large:-null}" \
  --argjson launch "$launch" \
  --argjson launch_small "${launch_small:-null}" \
  --argjson rss "${rss_bytes:-0}" \
  --arg profile "$PROFILE" \
  '{
     source: "real-app",
     profile: $profile,
     prewindow: $prewindow,
     phases: { "repo-10": $phases_small, "repo-5000": $phases_large },
     launch: $launch,
     launch_small_repo: $launch_small,
     peak_rss_bytes: $rss
   }'
