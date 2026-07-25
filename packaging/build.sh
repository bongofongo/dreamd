#!/usr/bin/env bash
#
# Build, sign, notarize, staple and package dreamd for ONE target.
#
#   packaging/build.sh aarch64-apple-darwin
#   NO_SIGN=1 packaging/build.sh aarch64-apple-darwin    # local, unsigned
#
# This script is the whole release pipeline. `.github/workflows/release.yml` is
# a thin wrapper around it, so a release can be reproduced and debugged locally
# without pushing a tag.
#
# Signing is driven entirely by the environment (APPLE_SIGNING_IDENTITY,
# APPLE_ID, APPLE_PASSWORD, APPLE_TEAM_ID, and in CI APPLE_CERTIFICATE /
# APPLE_CERTIFICATE_PASSWORD, which the Tauri bundler imports itself). An unset
# environment degrades to an unsigned build rather than a hard failure, so
# `NO_SIGN=1` is only needed to say so out loud.
#
# Everything platform-specific is derived from the target triple below, never
# from the CI matrix — that is what keeps adding a Linux target to one matrix
# entry plus one `case` arm.
set -euo pipefail

TARGET="${1:?usage: build.sh <rust-target-triple>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$("$ROOT/packaging/version.sh")"
DIST="$ROOT/dist"
NAME="dreamd-$VERSION-$TARGET"

case "$TARGET" in
  *-apple-darwin)
    # No dmg. Tauri's dmg bundler shells out to an osascript that asks Finder to
    # position the icons; without Automation->Finder permission it dies with
    # "AppleEvent timed out (-1712)", which is a coin flip on a CI runner. The
    # .zip is what both the Homebrew cask and install.sh consume anyway.
    BUNDLES="app"
    ;;
  *-unknown-linux-gnu)
    BUNDLES="appimage,deb"
    ;;
  *)
    echo "build.sh: unknown target $TARGET" >&2
    exit 1
    ;;
esac

echo "==> dreamd $VERSION for $TARGET"

ARGS=(--target "$TARGET" --bundles "$BUNDLES" --ci)
[[ -n "${NO_SIGN:-}" ]] && ARGS+=(--no-sign)
( cd "$ROOT" && cargo tauri build "${ARGS[@]}" )

mkdir -p "$DIST"
BUNDLE="$ROOT/target/$TARGET/release/bundle"

case "$TARGET" in
  *-apple-darwin)
    APP="$BUNDLE/macos/dreamd.app"
    [[ -d "$APP" ]] || { echo "build.sh: no .app at $APP" >&2; exit 1; }

    if [[ -z "${NO_SIGN:-}" ]]; then
      # Notarization is not optional and it is not visually obvious when it
      # silently no-ops — a misspelled env var yields an unsigned build that
      # only fails on a *user's* machine, as "dreamd is damaged". Fail here
      # instead.
      echo "==> verifying signature and notarization"
      codesign --verify --deep --strict --verbose=2 "$APP"
      spctl -a -vvv -t exec "$APP"
      xcrun stapler validate "$APP"
    fi

    # ditto, not tar. Part of a .app's code signature lives in extended
    # attributes; tar drops them and the extracted app arrives unsigned, which
    # fails Gatekeeper in a way that looks exactly like a notarization problem.
    # ditto writes a zip, which is also what Apple's own notarization tooling
    # expects — so the artifact is .zip and install.sh unpacks with `ditto -x`.
    echo "==> packaging $NAME.zip"
    rm -f "$DIST/$NAME.zip"
    ( cd "$(dirname "$APP")" && ditto -c -k --sequesterRsrc --keepParent "$(basename "$APP")" "$DIST/$NAME.zip" )
    ;;
  *-unknown-linux-gnu)
    echo "build.sh: linux packaging not written yet" >&2
    exit 1
    ;;
esac

# The checksums are the artifacts of record: the cask reads these files rather
# than re-downloading and re-hashing, so its sha256s are provably the same bytes
# that were signed here.
( cd "$DIST" && for f in "$NAME".*; do
    [[ "$f" == *.sha256 ]] && continue
    shasum -a 256 "$f" | awk '{print $1}' > "$f.sha256"
    echo "    $f  $(cat "$f.sha256")"
  done )

echo "==> done: $DIST"
