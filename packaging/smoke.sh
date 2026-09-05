#!/usr/bin/env bash
#
# Launch dreamd on a headless Linux box and assert it actually came up.
#
# Every other check in this repo stops at "it compiles and the logic is sound":
# `cargo test` never touches a window, `ui-check.mjs` drives Chromium behind a
# stub of the Rust commands, and `package-smoke` only asserts the artifacts are
# non-empty files. Nothing ran the program. That is the gap this closes — and it
# is the gap that matters most off macOS, because the failures unique to Linux
# (GTK init, the webview's renderer, a missing runtime dependency in a .deb) all
# happen *before* any of those checks would have an opinion.
#
# Two assertion modes, because two very different binaries need smoking:
#
#   SMOKE_EXPECT=paint   For a build with `--features perf`. Waits for the
#     `first_paint` mark, which `ui/app.js` emits only after GTK initialised,
#     wry built a real WebKitGTK webview, `frontendDist` loaded, the CSP admitted
#     both classic scripts, and an IPC round trip completed in *both* directions.
#     One line of NDJSON standing in for the entire startup path. Strictly the
#     stronger check — use it wherever the binary can carry the feature.
#
#     Paint mode launches on a *file*, not the directory, and then performs one
#     Neovim-style atomic save (temp sibling + rename, `loop.sh`'s pattern) and
#     waits for the `d:save_to_paint` mark. That mark only exists if the watcher
#     saw the rename, coalesced it, the event crossed to the page, the frontend
#     matched it to the open document, and the full re-render — the staged
#     write, both IPC round trips, highlight re-application — completed. It is
#     the core product loop, asserted end to end in the real app, and it is the
#     one place the staged paint's frame-timing code runs under WebKitGTK in CI
#     rather than under Chromium.
#
#   SMOKE_EXPECT=window  For a release artifact, which carries no instrumentation
#     and must be smoked exactly as shipped. Three signals together: the MCP
#     socket appears under the (sandboxed) config dir, which `main`'s `.setup`
#     hook binds *after* the window exists; a `WebKit*` process is running as a
#     descendant, which means the webview was really constructed and not merely
#     asked for; and the process is still alive. Weaker than `paint` — it does
#     not prove the page executed — but it is the strongest thing available
#     against an uninstrumented binary, and it catches the whole class of
#     failures that kill dreamd before a window exists (see src/webkit.rs).
#
# The script is self-contained on purpose: it makes its own fixture repo, points
# XDG_CONFIG_HOME at a scratch directory so tenet 2 stays true of a CI run, and
# needs nothing but bash, coreutils and /proc. That is what lets it run inside a
# bare `fedora:latest` container next to a downloaded .tar.gz with no checkout.
#
# Usage:
#   packaging/smoke.sh <command> [args...]      # e.g. ./target/debug/dreamd
#
# Environment:
#   SMOKE_EXPECT=paint|window   what counts as "up" (default: window)
#   SMOKE_TIMEOUT=<seconds>     how long to wait for it (default: 90)
#   SMOKE_XVFB=0                do not re-exec under xvfb-run
#
# It does NOT cover the NVIDIA/Wayland abort `src/webkit.rs` works around: that
# needs the proprietary driver and a compositor, and no hosted runner has either.
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "usage: $0 <command> [args...]" >&2
  exit 2
fi

EXPECT="${SMOKE_EXPECT:-window}"
TIMEOUT="${SMOKE_TIMEOUT:-90}"

case "$EXPECT" in
  paint|window) ;;
  *) echo "SMOKE_EXPECT must be 'paint' or 'window', got '$EXPECT'" >&2; exit 2 ;;
esac

# Re-exec under Xvfb rather than launching Xvfb ourselves: xvfb-run picks a free
# display number, and doing it here means every child — including the WebKit
# processes we later look for — is inside the same process tree we can kill.
# Skipped when a display already exists, so this is also runnable on a desktop.
if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ] && [ "${SMOKE_XVFB:-1}" != "0" ]; then
  exec xvfb-run -a --server-args="-screen 0 1280x1024x24" "$0" "$@"
fi

WORK="$(mktemp -d)"
PID=""

cleanup() {
  if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    kill -KILL "$PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

# --- fixture --------------------------------------------------------------
#
# Small, but not empty: a heading for the outline, a link for the scheme guard,
# and a fenced block so syntect is on the path too. A blank document would
# render without exercising any of it.
FIXTURE="$WORK/repo"
mkdir -p "$FIXTURE"
# A fake .git, the way loop.sh makes one: a *file* launch resolves the repo by
# walking up from the cwd, and without a repo there is no watcher — the save
# assertion below would wait on a directory nobody is watching. (A directory
# launch takes the argument as root regardless, so window mode never needed
# this.)
mkdir -p "$FIXTURE/.git"
printf 'ref: refs/heads/main\n' > "$FIXTURE/.git/HEAD"
cat > "$FIXTURE/README.md" <<'MD'
# smoke

A paragraph with a [link](https://example.com) in it.

```rust
fn main() {
    println!("hello");
}
```

## Second heading

More text, so the outline has two entries.
MD
cat > "$FIXTURE/notes.md" <<'MD'
# notes

A second file, so the tree has something to walk.
MD

# Tenet 2 says dreamd writes only under ~/.config/dreamd. A CI run must not
# write into the *real* one — and pointing XDG_CONFIG_HOME here is also what
# makes the socket check below unambiguous, since the directory starts empty.
export XDG_CONFIG_HOME="$WORK/config"
mkdir -p "$XDG_CONFIG_HOME"

# Software rendering, no accessibility bus, no GPU. Each is a default, not an
# override: a caller that has already made a decision (the CI matrix arm that
# deliberately leaves the DMA-BUF renderer *on*, say) keeps it.
export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"
export WEBKIT_DISABLE_COMPOSITING_MODE="${WEBKIT_DISABLE_COMPOSITING_MODE:-1}"
export NO_AT_BRIDGE="${NO_AT_BRIDGE:-1}"
if [ -z "${WAYLAND_DISPLAY:-}" ]; then
  export GDK_BACKEND="${GDK_BACKEND:-x11}"
fi

LOG="$WORK/dreamd.log"

# Paint mode opens the document so the whole render pipeline is on the boot
# path and the save-loop assertion below has an open file to save; window mode
# keeps the directory launch a release artifact has always been smoked with.
if [ "$EXPECT" = "paint" ]; then
  TARGET="$FIXTURE/README.md"
else
  TARGET="$FIXTURE"
fi

echo "smoke: expect=$EXPECT timeout=${TIMEOUT}s"
echo "smoke: $* $TARGET"

# Launched from inside the fixture, because a *file* argument roots the tree —
# and arms the watcher — at the repo of the current directory; launched from
# the checkout, the save below would land in a directory nobody is watching.
# The command is absolutized first so `./target/debug/dreamd` survives the cd,
# and `exec` keeps $! the app's pid rather than the subshell's.
CMD0="$1"; shift
case "$CMD0" in
  */*) [ -e "$CMD0" ] && CMD0="$(cd "$(dirname "$CMD0")" && pwd)/$(basename "$CMD0")" ;;
esac
( cd "$FIXTURE" && exec "$CMD0" "$@" "$TARGET" ) > "$LOG" 2>&1 &
PID=$!

# --- assertions -----------------------------------------------------------

# `first_paint` is emitted by ui/app.js and reaches stderr through the
# `perf_mark` command. Match on the phase field rather than the whole record so
# a change to the NDJSON's other keys does not silently stop matching.
saw_first_paint() {
  grep -q '"phase":"first_paint"' "$LOG"
}

# Walk up /proc's ppid chain looking for our own pid. WebKitGTK spawns its web
# process directly today, but its sandbox launches through bwrap on some builds,
# which puts a generation in between — so this walks rather than comparing once.
is_descendant() {
  local pid="$1" hops=0 ppid
  while [ "$pid" -gt 1 ] && [ "$hops" -lt 12 ]; do
    [ -r "/proc/$pid/stat" ] || return 1
    # /proc/pid/stat's second field is the comm, parenthesised, and may itself
    # contain spaces and brackets. Everything after the final ')' is positional,
    # where field 1 is the state and field 2 is the ppid.
    ppid="$(sed 's/.*) //' "/proc/$pid/stat" | cut -d' ' -f2)"
    [ -n "$ppid" ] || return 1
    [ "$ppid" = "$PID" ] && return 0
    pid="$ppid"
    hops=$((hops + 1))
  done
  return 1
}

# /proc/pid/comm is capped at 15 characters, so "WebKitWebProcess" arrives
# truncated to "WebKitWebProces". Match the prefix, never the full name.
saw_webkit_process() {
  local dir comm
  for dir in /proc/[0-9]*; do
    [ -r "$dir/comm" ] || continue
    read -r comm < "$dir/comm" 2>/dev/null || continue
    case "$comm" in
      WebKit*) ;;
      *) continue ;;
    esac
    if is_descendant "${dir#/proc/}"; then
      return 0
    fi
  done
  return 1
}

# Bound in main's `.setup` hook, which Tauri runs after it has created the
# window declared in tauri.conf.json — so the socket existing is downstream of
# a window existing. The name is a hash of the repo root; the directory is
# scratch, so any .sock in it is ours.
saw_socket() {
  compgen -G "$XDG_CONFIG_HOME/dreamd/run/*.sock" > /dev/null
}

is_up() {
  if [ "$EXPECT" = "paint" ]; then
    saw_first_paint
  else
    saw_socket && saw_webkit_process
  fi
}

fail() {
  echo "smoke: FAILED — $1" >&2
  echo "--- output -------------------------------------------------------" >&2
  cat "$LOG" >&2 || true
  echo "------------------------------------------------------------------" >&2
  exit 1
}

waited=0
until is_up; do
  if ! kill -0 "$PID" 2>/dev/null; then
    wait "$PID" 2>/dev/null && code=0 || code=$?
    fail "the process exited during startup (status $code)"
  fi
  if [ "$waited" -ge "$((TIMEOUT * 4))" ]; then
    fail "no sign of a running window after ${TIMEOUT}s"
  fi
  sleep 0.25
  waited=$((waited + 1))
done

echo "smoke: up after ~$((waited / 4))s"

# Only the unambiguously fatal patterns. GTK and GLib emit CRITICALs under Xvfb
# for reasons that have nothing to do with dreamd, and failing on those would
# make this job a source of noise rather than of signal — they are printed
# below instead, where a human reading a red build can still see them.
if grep -Eq 'Gdk-Message: Error|panicked at|Protocol error|Segmentation fault' "$LOG"; then
  fail "a fatal message appeared in the output"
fi

# --- the core loop, once -------------------------------------------------
#
# Paint mode only: `d:save_to_paint` is a perf mark, and window mode has none.
# One atomic save of the open document, the way Neovim's `backupcopy=auto`
# writes one, then wait for the mark that only a completed re-render emits.
# This is the integration test the launch alone is not: watcher -> event ->
# frontend match -> render + reanchor round trips -> staged write -> highlight
# pass, all in the real app. Thirty seconds is generous — the loop measures in
# the hundreds of ms — so a timeout here is a wedge, not a slow machine.
if [ "$EXPECT" = "paint" ]; then
  sed 's/^More text.*/More text, edited once by the smoke test./' \
    "$FIXTURE/README.md" > "$FIXTURE/README.md.tmp"
  mv "$FIXTURE/README.md.tmp" "$FIXTURE/README.md"
  waited=0
  until grep -q '"phase":"d:save_to_paint"' "$LOG"; do
    if ! kill -0 "$PID" 2>/dev/null; then
      fail "the process died during the save"
    fi
    if [ "$waited" -ge 120 ]; then
      fail "the save never repainted (no d:save_to_paint after 30s)"
    fi
    sleep 0.25
    waited=$((waited + 1))
  done
  echo "smoke: one save repainted after ~$((waited / 4))s"
fi

# --- shutdown -------------------------------------------------------------
#
# A process that ignores SIGTERM is a real bug — dreamd flushes marks on
# ExitRequested, and a hang here means that path is wedged — so this asserts the
# exit happened rather than reaching straight for SIGKILL. The *status* is not
# asserted: unhandled SIGTERM is a perfectly good way to die.
kill -TERM "$PID" 2>/dev/null || true
for _ in $(seq 1 40); do
  kill -0 "$PID" 2>/dev/null || break
  sleep 0.25
done
if kill -0 "$PID" 2>/dev/null; then
  fail "still running 10s after SIGTERM"
fi
wait "$PID" 2>/dev/null && status=0 || status=$?
PID=""

echo "smoke: exited on SIGTERM (status $status)"
if grep -Eq 'CRITICAL|WARNING \*\*' "$LOG"; then
  echo "smoke: note — GLib/GTK diagnostics were emitted (not fatal):"
  grep -E 'CRITICAL|WARNING \*\*' "$LOG" | sort -u | head -20
fi
echo "smoke: OK"
