#!/usr/bin/env bash
#
# Assert that the tree's version matches the tag being released.
#
#   packaging/check-version.sh 0.2.0        # i.e. "${GITHUB_REF_NAME#v}"
#
# CI asserts rather than rewrites: a tag should describe a commit, not change
# it. This runs before the build matrix, so a mismatched tag costs ~15 seconds
# instead of producing artifacts whose filenames disagree with their contents.
set -euo pipefail

WANT="${1:?usage: check-version.sh <x.y.z>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fail=0

got="$("$ROOT/packaging/version.sh")"
if [[ "$got" != "$WANT" ]]; then
  echo "src-tauri/Cargo.toml says $got, tag says $WANT" >&2
  fail=1
fi

# Anchored to the start of the line, so this cannot match some other const that
# happens to mention a version. set-version.sh reads it back with the same
# expression, which is what keeps the writer and the asserter agreeing.
site="$(sed -n 's/^export const VERSION = "\([^"]*\)".*/\1/p' "$ROOT/website/src/consts.ts")"
if [[ "$site" != "$WANT" ]]; then
  echo "website/src/consts.ts says $site, tag says $WANT" >&2
  fail=1
fi

if [[ $fail -ne 0 ]]; then
  echo "run: packaging/set-version.sh $WANT && cargo build" >&2
  exit 1
fi

echo "version $WANT agrees across the tree"
