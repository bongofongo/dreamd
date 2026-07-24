#!/usr/bin/env bash
#
# Deep profiling — the `deep` tier only.
#
#   perf/scripts/profile.sh
#
# Everything here needs a symbolicated binary, so it builds with the
# `profiling` profile (release codegen, symbols kept) rather than `release`.
#
# Each tool is optional. A missing one prints its install line and is skipped,
# the way the README documents `cargo install tauri-cli` — a deep run should
# still produce whatever it can rather than failing outright.
#
# Artifacts land in perf/results/ (gitignored). Machine-readable summary on
# stdout, human narration on stderr.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

RESULTS="$ROOT/perf/results"
mkdir -p "$RESULTS"
STAMP="$(git rev-parse --short HEAD 2>/dev/null || echo nogit)"

CORPUS="$ROOT/perf/corpus/generated"
DOC="$CORPUS/docs/code-2m.md"
LARGE_REPO="$CORPUS/repos/repo-5000"

have() { command -v "$1" >/dev/null 2>&1; }
skip() { echo "  skipped: $1 not installed — $2" >&2; }

echo "building --profile profiling --features perf..." >&2
cargo build --profile profiling --features perf >&2
BIN="$ROOT/target/profiling/dreamd"

# Confirm symbols actually survived; a silently-stripped binary would produce
# a trace full of hex addresses and waste the whole run.
#
# `grep -c` rather than `grep -q`: under `set -o pipefail`, `grep -q` exits as
# soon as it matches, `nm` then dies of SIGPIPE, and the pipeline reports
# failure even though the match succeeded — which reported every symbolicated
# binary as stripped. `-c` consumes all input, so nothing gets a broken pipe.
sym_count="$(nm "$BIN" 2>/dev/null | grep -c 'dreamd' || true)"
if [[ "${sym_count:-0}" -gt 0 ]]; then
  symbols=true
else
  symbols=false
  echo "  WARNING: no dreamd symbols found in $BIN — traces will not symbolicate" >&2
fi

# ---- release binary size + composition -----------------------------------

echo "building --release for size measurement..." >&2
cargo build --release >&2
size_bytes="$(stat -f%z "$ROOT/target/release/dreamd")"

bloat_json='null'
if have cargo-bloat; then
  echo "cargo bloat..." >&2
  bloat_json="$(cargo bloat --release --crates -n 15 --message-format json 2>/dev/null \
    | jq -c '{crates: [.crates[]? | {name, size}]}' 2>/dev/null || echo 'null')"
else
  skip "cargo-bloat" "cargo install cargo-bloat"
fi

# ---- samply: flamegraph of the render path -------------------------------

samply_out='null'
if have samply; then
  echo "samply: profiling render of code-2m.md..." >&2
  prof="$RESULTS/samply-render-$STAMP.json.gz"
  # `--save-only` keeps it from opening a browser mid-run.
  if samply record --save-only -o "$prof" -- \
      "$ROOT/target/profiling/examples/render_doc" "$DOC" /dev/null >/dev/null 2>&1; then
    samply_out="$(jq -n --arg p "$prof" '{profile: $p, view: "samply load \($p)"}')"
  else
    # The example needs building under the profiling profile too.
    cargo build --profile profiling --example render_doc >&2 || true
    if samply record --save-only -o "$prof" -- \
        "$ROOT/target/profiling/examples/render_doc" "$DOC" /dev/null >/dev/null 2>&1; then
      samply_out="$(jq -n --arg p "$prof" '{profile: $p, view: "samply load \($p)"}')"
    else
      echo "  samply record failed (macOS may need permission to sample)" >&2
    fi
  fi
else
  skip "samply" "brew install samply"
fi

# ---- Instruments: App Launch + Time Profiler -----------------------------

xctrace_out='[]'
if have xctrace; then
  traces=()
  for template in "Time Profiler"; do
    slug="$(echo "$template" | tr ' A-Z' '-a-z')"
    out="$RESULTS/$slug-$STAMP.trace"
    rm -rf "$out"
    echo "xctrace: $template..." >&2
    if xctrace record --template "$template" --output "$out" \
         --launch -- "$BIN" --bench-startup "$LARGE_REPO" >/dev/null 2>&1; then
      traces+=("$out")
    else
      echo "  xctrace '$template' failed (Instruments may need Developer Mode enabled)" >&2
    fi
  done
  if (( ${#traces[@]} )); then
    xctrace_out="$(printf '%s\n' "${traces[@]}" | jq -R . | jq -s .)"
  fi
else
  skip "xctrace" "install Xcode"
fi

# ---- /usr/bin/sample fallback --------------------------------------------
# Always available, zero install. Useful when the above are unavailable.

sample_out='null'
if [[ "$samply_out" == "null" ]] && [[ -x /usr/bin/sample ]]; then
  echo "falling back to /usr/bin/sample..." >&2
  out="$RESULTS/sample-$STAMP.txt"
  "$BIN" "$DOC" >/dev/null 2>&1 &
  pid=$!
  sleep 1
  /usr/bin/sample "$pid" 3 -file "$out" >/dev/null 2>&1 || true
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  [[ -f "$out" ]] && sample_out="$(jq -n --arg p "$out" '{report: $p}')"
fi

jq -n \
  --argjson size "$size_bytes" \
  --argjson symbols "$symbols" \
  --argjson bloat "$bloat_json" \
  --argjson samply "$samply_out" \
  --argjson traces "$xctrace_out" \
  --argjson sample "$sample_out" \
  '{
     source: "real-app",
     release_binary_bytes: $size,
     symbolicated: $symbols,
     bloat: $bloat,
     samply: $samply,
     instruments_traces: $traces,
     sample_fallback: $sample
   }'
