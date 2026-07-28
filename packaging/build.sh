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
    # If the AppImage step dies with a bare `failed to run linuxdeploy`, set
    # NO_STRIP=1 and run it again. linuxdeploy carries its own binutils `strip`,
    # too old to parse the SHT_RELR sections (`.relr.dyn`, type 0x13) that
    # distributions now emit, so it fails on every system library it copies in.
    # Tauri swallows linuxdeploy's stderr, which is why the error says nothing.
    # NO_STRIP is read by linuxdeploy straight out of the environment, so it
    # needs no plumbing here. ubuntu-22.04 predates RELR and never trips it,
    # which is why release.yml does not set it.
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
# VERBOSE=1 is the only way to see *why* linuxdeploy failed. The bundler runs it
# through `cmd.output()` — stderr captured and dropped — whenever its log level
# is Error, and only switches to the variant that surfaces the child's output
# when it is not. So the bare `failed to run linuxdeploy` above is not a terse
# error, it is the complete one, and a rerun with this set is the only way to
# turn it into a sentence. canary.yml sets it always, because a scheduled job
# nobody is watching gets exactly one chance to explain itself.
[[ -n "${VERBOSE:-}" ]] && ARGS+=(--verbose)
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
    # Nothing to verify: there is no Gatekeeper, no notarization ticket and no
    # signature to check, so the codesign/spctl/stapler block above has no Linux
    # counterpart. The bundler's own output is the artifact.
    APPIMAGE="$(find "$BUNDLE/appimage" -maxdepth 1 -name '*.AppImage' -print -quit 2>/dev/null || true)"
    DEB="$(find "$BUNDLE/deb" -maxdepth 1 -name '*.deb' -print -quit 2>/dev/null || true)"
    [[ -n "$APPIMAGE" ]] || { echo "build.sh: no .AppImage under $BUNDLE/appimage" >&2; exit 1; }
    [[ -n "$DEB" ]]      || { echo "build.sh: no .deb under $BUNDLE/deb" >&2; exit 1; }

    echo "==> packaging $NAME.AppImage, $NAME.deb, $NAME.tar.gz"
    rm -f "$DIST/$NAME.AppImage" "$DIST/$NAME.deb" "$DIST/$NAME.tar.gz"
    cp "$APPIMAGE" "$DIST/$NAME.AppImage"
    cp "$DEB" "$DIST/$NAME.deb"

    # The third artifact is a plain FHS tree, and it is not redundant: one binary
    # is both the GUI and the CLI (`dreamd mcp`, `theme`, `config`, `marks`), so
    # a headless session, a container, install.sh and the PKGBUILD all want it
    # without an AppImage's FUSE requirement or dpkg. tar is correct here in a
    # way it is not on macOS — an ELF carries no signature in xattrs.
    #
    # It is tarred from the .deb's *staging* tree rather than from the release
    # binary directly. The bundler leaves that tree in place after packing, and
    # it already holds the generated .desktop entry and the hicolor icons — so
    # the tarball, the .deb and the AppImage all carry byte-identical desktop
    # integration instead of this script inventing a second copy that drifts.
    DEBDATA="$(find "$BUNDLE/deb" -maxdepth 2 -type d -name data -print -quit 2>/dev/null || true)"
    [[ -n "$DEBDATA" && -x "$DEBDATA/usr/bin/dreamd" ]] \
      || { echo "build.sh: no staged deb tree with usr/bin/dreamd under $BUNDLE/deb" >&2; exit 1; }
    tar -czf "$DIST/$NAME.tar.gz" -C "$DEBDATA" usr
    ;;
esac

# The checksums are the artifacts of record: the cask and the PKGBUILD read
# these files rather than re-downloading and re-hashing, so their sha256s are
# provably the same bytes that were built here.
#
# `shasum` is perl's and ships with macOS; most Linux images have `sha256sum`
# from coreutils and not `shasum` at all. Both print "<hash>  <name>".
if command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1"; }
else
  sha256() { sha256sum "$1"; }
fi

( cd "$DIST" && for f in "$NAME".*; do
    [[ "$f" == *.sha256 ]] && continue
    sha256 "$f" | awk '{print $1}' > "$f.sha256"
    echo "    $f  $(cat "$f.sha256")"
  done )

echo "==> done: $DIST"
