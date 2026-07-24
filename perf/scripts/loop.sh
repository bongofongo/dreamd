#!/usr/bin/env bash
#
# The core product loop: one save in Neovim through to a re-anchored, repainted
# document. This is the number that matters most, because it is the one the user
# hits dozens of times an hour.
#
#   perf/scripts/loop.sh [--release] [--highlights N] [--saves M]
#
# Two findings come out of this, and they are independent:
#
#   events_per_save   How many `file-changed` events one atomic save produces.
#                     Anything above 1.0 is the missing watcher debounce (fix
#                     B2) — macOS FSEvents emits several per save and
#                     `watcher.rs` forwards each one, so the document is
#                     re-rendered two or three times for a single :w.
#
#   save_to_paint     Wall time from the event arriving in JS to highlights
#                     re-applied, p50/p95. Scales with highlight count via
#                     `reanchor`, which is why --highlights matters.
#
# Runs against a scratch copy of the corpus in a throwaway repo, never the
# user's files.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

PROFILE_ARGS=()
BIN="target/debug/dreamd"
PROFILE="debug"
HIGHLIGHTS=10
SAVES=12
while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) PROFILE_ARGS=(--release); BIN="target/release/dreamd"; PROFILE="release"; shift ;;
    --highlights) HIGHLIGHTS="$2"; shift 2 ;;
    --saves) SAVES="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

CORPUS="$ROOT/perf/corpus/generated"
SRC_DOC="$CORPUS/docs/mixed-2m.md"
SEED="$CORPUS/highlights/$HIGHLIGHTS.json"

if [[ ! -f "$SRC_DOC" ]]; then
  echo "corpus missing — run: node perf/corpus/gen.mjs" >&2
  exit 1
fi
if [[ ! -f "$SEED" ]]; then
  echo "no highlight fixture for N=$HIGHLIGHTS (have: 1 10 100 500)" >&2
  exit 2
fi

if [[ "${DREAMD_PERF_NO_WINDOW:-0}" == "1" ]]; then
  echo "DREAMD_PERF_NO_WINDOW=1 — loop.sh needs a window; skipping." >&2
  jq -n --arg profile "$PROFILE" '{source: "real-app", profile: $profile, skipped: "no-window mode"}'
  exit 0
fi

echo "building (--features perf ${PROFILE_ARGS[*]:-})..." >&2
cargo build --features perf "${PROFILE_ARGS[@]}" >&2

# ---- scratch repo --------------------------------------------------------

WORK="$(mktemp -d)"
cleanup() {
  [[ -n "${APP_PID:-}" ]] && kill "$APP_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

mkdir -p "$WORK/.git"
printf 'ref: refs/heads/main\n' >"$WORK/.git/HEAD"
DOC="$WORK/doc.md"
cp "$SRC_DOC" "$DOC"

STREAM="$WORK/stream.ndjson"

echo "launching with $HIGHLIGHTS highlights; $SAVES saves..." >&2
# Launched from inside the scratch repo on purpose. `resolve_target` roots the
# tree at the repo of the *current directory* when the argument is a file
# (README: "passing a file opens it, but the file tree is still rooted at the
# repo of the current directory"), and the watcher follows the root. Launching
# from the dreamd repo would watch the dreamd repo and never see these saves.
#
# `exec` matters: without it `$!` is the subshell's pid, so the kill at the end
# hits the subshell and leaves a live dreamd process behind on every run.
( cd "$WORK" && DREAMD_PERF_SEED="$SEED" exec "$ROOT/$BIN" "$DOC" >/dev/null 2>"$STREAM" ) &
APP_PID=$!

# Wait for the first paint before touching anything, or the early saves race
# the initial render and the numbers are meaningless.
waited=0
until grep -q '"first_paint"' "$STREAM" 2>/dev/null; do
  kill -0 "$APP_PID" 2>/dev/null || { echo "app exited during startup:" >&2; cat "$STREAM" >&2; exit 1; }
  sleep 0.1
  waited=$((waited + 1))
  if (( waited > 300 )); then
    echo "timed out waiting for first_paint (30s)" >&2
    cat "$STREAM" >&2
    exit 1
  fi
done

# ---- drive the saves -----------------------------------------------------
# Rewrite via a temp file + mv, which is what Neovim's default `backupcopy=auto`
# does. That atomic-replace pattern is exactly what makes FSEvents emit a burst,
# so simulating it faithfully matters.

for i in $(seq 1 "$SAVES"); do
  {
    cat "$SRC_DOC"
    printf '\n<!-- edit %s -->\n' "$i"
  } >"$DOC.tmp"
  mv "$DOC.tmp" "$DOC"
  sleep 1.2
done

sleep 1
kill "$APP_PID" 2>/dev/null || true
wait "$APP_PID" 2>/dev/null || true
APP_PID=""

# ---- report --------------------------------------------------------------

# The app's stderr carries more than the NDJSON marks — the seeding notice, and
# whatever WebKit feels like logging. Anything non-JSON would abort jq, so the
# stream is filtered to perf records before parsing.
grep '"dreamd_perf"' "$STREAM" | jq -s --argjson saves "$SAVES" --argjson highlights "$HIGHLIGHTS" --arg profile "$PROFILE" '
  [.[] | select(type == "object" and .dreamd_perf == 1)] as $marks
  | ($marks | map(select(.phase == "watcher_event")) | length) as $events
  | ($marks | map(select(.phase == "d:save_to_paint") | .ms) | sort) as $paints
  | ($marks | map(select(.phase == "d:ipc_reanchor")  | .ms) | sort) as $reanchor
  | ($marks | map(select(.phase == "d:ipc_render_markdown") | .ms) | sort) as $render
  | ($marks | map(select(.phase == "d:apply_highlights") | .ms) | sort) as $apply
  | def pct(a; p): if (a | length) == 0 then null
                   else a[ (((a|length) - 1) * p) | floor ] end;
  {
    source: "real-app",
    profile: $profile,
    highlights: $highlights,
    saves: $saves,
    watcher_events: $events,
    # >1.0 means one save is re-rendering the document more than once.
    events_per_save: (if $saves > 0 then ($events / $saves) else null end),
    repaints: ($paints | length),
    save_to_paint_ms:      { p50: pct($paints;   0.5), p95: pct($paints;   0.95), max: ($paints   | max) },
    ipc_reanchor_ms:       { p50: pct($reanchor; 0.5), p95: pct($reanchor; 0.95) },
    ipc_render_markdown_ms:{ p50: pct($render;   0.5), p95: pct($render;   0.95) },
    apply_highlights_ms:   { p50: pct($apply;    0.5), p95: pct($apply;    0.95) }
  }
'
