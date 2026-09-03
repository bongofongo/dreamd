#!/usr/bin/env bash
#
# Append (or replace) one release's numbers in the public, website-facing
# performance history at website/src/data/perf-history.json.
#
#   perf/scripts/publish-history.sh <deep-tier-result.json> <appimage-file>
#
# The source is a `perf/run.sh deep` result (perf/results/deep-*.json) and the
# real shipped AppImage from `packaging/build.sh` — this script reads neither
# path itself, both are handed in because they come from two separate runs.
#
# Unlike perf/results/*.json (150+ metrics, gitignored, shaped around a
# private per-machine baseline) this output is small, public, and tracked: it
# is the seed for a chart on the website, not a performance report. Four
# numbers per release, nothing else.
#
# Idempotent by `version`: re-running with numbers for a version already in
# the file replaces that entry in place rather than duplicating it, so a
# corrected re-run is safe and git history keeps the old value. A new version
# is appended to the end — entries are meant to stay oldest-first, and this
# script never reorders what's already there.
set -euo pipefail

usage() {
  echo "usage: perf/scripts/publish-history.sh <deep-tier-result.json> <appimage-file>" >&2
  exit 1
}

RESULT="${1:-}"
APPIMAGE="${2:-}"
[[ -n "$RESULT" && -n "$APPIMAGE" ]] || usage

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/website/src/data/perf-history.json"

command -v jq >/dev/null 2>&1 || {
  echo "publish-history.sh: jq is required (perf/run.sh needs it too)" >&2; exit 1; }

[[ -r "$RESULT" ]] || { echo "publish-history.sh: can't read result file: $RESULT" >&2; exit 1; }
[[ -r "$APPIMAGE" ]] || { echo "publish-history.sh: can't read AppImage: $APPIMAGE" >&2; exit 1; }

jq empty "$RESULT" 2>/dev/null || {
  echo "publish-history.sh: $RESULT is not valid JSON" >&2; exit 1; }

VERSION="$("$ROOT/packaging/version.sh")"
SHA="$(jq -r '.meta.sha // empty' "$RESULT")"
[[ -n "$SHA" ]] || { echo "publish-history.sh: $RESULT has no .meta.sha" >&2; exit 1; }

# Same <os>-<arch>-<hostname> id notes/perf-baselines/ keys results by — a
# deep result already carries it, so this is a read, not a new convention.
RUNNER="$(jq -r '.meta.machine.id // empty' "$RESULT")"
[[ -n "$RUNNER" ]] || { echo "publish-history.sh: $RESULT has no .meta.machine.id" >&2; exit 1; }

DATE="$(date -u +%Y-%m-%d)"

# Pull each required metric individually so a missing one names itself rather
# than the whole entry silently landing as null.
first_paint="$(jq -r '.real.startup.release.launch.first_paint // empty' "$RESULT")"
save_p50="$(jq -r '.real.loop["release-h100"].save_to_paint_ms.p50 // empty' "$RESULT")"
peak_rss_bytes="$(jq -r '.real.startup.release.peak_rss_bytes // empty' "$RESULT")"

missing=()
[[ -n "$first_paint" ]]     || missing+=("real.startup.release.launch.first_paint")
[[ -n "$save_p50" ]]        || missing+=('real.loop["release-h100"].save_to_paint_ms.p50')
[[ -n "$peak_rss_bytes" ]]  || missing+=("real.startup.release.peak_rss_bytes")
if [[ ${#missing[@]} -gt 0 ]]; then
  echo "publish-history.sh: $RESULT is missing required key(s):" >&2
  printf '  %s\n' "${missing[@]}" >&2
  echo "(is this a deep-tier result, run with --release?)" >&2
  exit 1
fi

appimage_bytes="$(stat -f%z "$APPIMAGE" 2>/dev/null || stat -c%s "$APPIMAGE")"

# Decimal MB (1,000,000), not MiB: nothing else in perf/ converts bytes to MB
# today (release_binary_bytes in profile.sh is reported raw), so this follows
# the convention this task specified rather than one that already existed.
peak_rss_mb="$(jq -n --argjson b "$peak_rss_bytes" '($b / 1000000 * 100 | round) / 100')"
appimage_mb="$(jq -n --argjson b "$appimage_bytes" '($b / 1000000 * 100 | round) / 100')"

mkdir -p "$(dirname "$OUT")"

if [[ ! -f "$OUT" ]]; then
  jq -n '{schema: 1, entries: []}' > "$OUT"
fi

jq empty "$OUT" 2>/dev/null || {
  echo "publish-history.sh: existing $OUT is not valid JSON, refusing to touch it" >&2; exit 1; }

ENTRY="$(jq -n \
  --arg version "$VERSION" \
  --arg sha "$SHA" \
  --arg date "$DATE" \
  --arg runner "$RUNNER" \
  --argjson first_paint_ms "$first_paint" \
  --argjson save_to_paint_p50_ms "$save_p50" \
  --argjson peak_rss_mb "$peak_rss_mb" \
  --argjson appimage_mb "$appimage_mb" \
  '{
    version: $version,
    sha: $sha,
    date: $date,
    runner: $runner,
    metrics: {
      first_paint_ms: $first_paint_ms,
      save_to_paint_p50_ms: $save_to_paint_p50_ms,
      peak_rss_mb: $peak_rss_mb,
      appimage_mb: $appimage_mb
    }
  }')"

EXISTED="$(jq --arg v "$VERSION" '[.entries[] | select(.version == $v)] | length > 0' "$OUT")"

TMP="$(mktemp "$OUT.XXXXXX")"
trap 'rm -f "$TMP"' EXIT

jq --arg v "$VERSION" --argjson entry "$ENTRY" '
  .schema = 1
  | .entries = (
      if any(.entries[]; .version == $v)
      then [.entries[] | if .version == $v then $entry else . end]
      else .entries + [$entry]
      end
    )
' "$OUT" > "$TMP"

mv "$TMP" "$OUT"
trap - EXIT

if [[ "$EXISTED" == "true" ]]; then
  echo "publish-history.sh: replaced existing entry for version $VERSION (sha $SHA)" >&2
else
  echo "publish-history.sh: appended new entry for version $VERSION (sha $SHA)" >&2
fi
