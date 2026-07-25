#!/bin/sh
#
# dreamd installer.
#
#   curl -fsSL https://raw.githubusercontent.com/bongofongo/dreamd/main/packaging/install.sh | sh
#
# Installs dreamd.app into /Applications and symlinks the CLI onto your PATH.
# They are the same binary: dreamd.app/Contents/MacOS/dreamd is both the window
# and the command line, so the two can never drift apart.
#
#   DREAMD_VERSION=v0.1.0 sh install.sh   # pin a version instead of latest
#   sh install.sh --uninstall             # remove the app and the symlink
#
# On quarantine, because the folklore is wrong and this script deliberately
# does NOT run `xattr -cr`: com.apple.quarantine is set by the *downloading
# application*, not by the OS at write time. curl does not set it; browsers do.
# So the archive this script fetches carries no quarantine attribute at all and
# Gatekeeper never runs on it. Stripping quarantine "just in case" would teach a
# reflex that defeats Gatekeeper on downloads that genuinely need checking.
set -eu

REPO="bongofongo/dreamd"
APP_NAME="dreamd.app"

die() { printf 'install.sh: %s\n' "$1" >&2; exit 1; }
say() { printf '%s\n' "$1"; }

# ---- uninstall -------------------------------------------------------------

if [ "${1:-}" = "--uninstall" ]; then
  for d in /Applications "$HOME/Applications"; do
    [ -d "$d/$APP_NAME" ] && rm -rf "$d/$APP_NAME" && say "removed $d/$APP_NAME"
  done
  for d in /usr/local/bin "$HOME/.local/bin" /opt/homebrew/bin; do
    [ -L "$d/dreamd" ] && rm -f "$d/dreamd" && say "removed $d/dreamd"
  done
  say "dreamd removed. Your config in ~/.config/dreamd was left alone."
  exit 0
fi

# ---- platform --------------------------------------------------------------

[ "$(uname -s)" = "Darwin" ] || die "macOS only for now — on Linux, build from source: https://github.com/$REPO"

# uname -m lies under Rosetta: a translated shell on an Apple Silicon Mac reports
# x86_64, and we would install the slower build on hardware that can run native.
arch="$(uname -m)"
if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
  arch="arm64"
fi
case "$arch" in
  arm64)  target="aarch64-apple-darwin" ;;
  x86_64) target="x86_64-apple-darwin" ;;
  *)      die "unsupported architecture: $arch" ;;
esac

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v ditto >/dev/null 2>&1 || die "ditto is required"

# ---- version ---------------------------------------------------------------

tag="${DREAMD_VERSION:-}"
if [ -z "$tag" ]; then
  tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
  [ -n "$tag" ] || die "could not determine the latest release — set DREAMD_VERSION to pin one"
fi
version="${tag#v}"

name="dreamd-$version-$target"
base="https://github.com/$REPO/releases/download/$tag"

say "==> dreamd $version ($target)"

# ---- download and verify ---------------------------------------------------

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

curl -fsSL "$base/$name.zip"        -o "$tmp/$name.zip"     || die "download failed: $base/$name.zip"
curl -fsSL "$base/$name.zip.sha256" -o "$tmp/$name.sha256"  || die "checksum download failed"

want="$(tr -d '[:space:]' < "$tmp/$name.sha256")"
got="$(shasum -a 256 "$tmp/$name.zip" | awk '{print $1}')"
[ "$want" = "$got" ] || die "checksum mismatch — expected $want, got $got"
say "    checksum ok"

# ditto, matching how it was packed: a .app's code signature lives partly in
# extended attributes, and unzip(1) drops them.
ditto -x -k "$tmp/$name.zip" "$tmp/x" || die "could not extract the archive"
[ -d "$tmp/x/$APP_NAME" ] || die "archive did not contain $APP_NAME"

# Cheap, and it turns a corrupted or tampered download into a clear error here
# rather than a Gatekeeper mystery two minutes from now.
if ! codesign --verify --deep --strict "$tmp/x/$APP_NAME" 2>/dev/null; then
  say "    warning: signature could not be verified (unsigned build?)"
fi

# ---- install ---------------------------------------------------------------

appdir=/Applications
if [ ! -w "$appdir" ]; then
  appdir="$HOME/Applications"
  mkdir -p "$appdir"
  say "    /Applications is not writable, using $appdir"
fi

# Remove first: ditto merges into an existing bundle rather than replacing it,
# which would leave orphaned files from the previous version behind.
rm -rf "$appdir/$APP_NAME"
ditto "$tmp/x/$APP_NAME" "$appdir/$APP_NAME" || die "could not install to $appdir"
say "    installed $appdir/$APP_NAME"

# ---- CLI symlink -----------------------------------------------------------

exe="$appdir/$APP_NAME/Contents/MacOS/dreamd"
bindir=""
for d in /usr/local/bin "$HOME/.local/bin" /opt/homebrew/bin; do
  if [ -d "$d" ] && [ -w "$d" ]; then bindir="$d"; break; fi
done

if [ -n "$bindir" ]; then
  ln -sf "$exe" "$bindir/dreamd"
  say "    linked $bindir/dreamd"
  case ":$PATH:" in
    *":$bindir:"*) ;;
    *) say "    note: $bindir is not on your PATH — add: export PATH=\"$bindir:\$PATH\"" ;;
  esac
else
  # Never escalate silently inside a curl | sh. Print the command and let the
  # user run it with their eyes open.
  say ""
  say "    No writable bin directory found. To put dreamd on your PATH, run:"
  say "        sudo ln -sf '$exe' /usr/local/bin/dreamd"
fi

say ""
say "Done. Run 'dreamd' inside a repo, or open the app and use File > Open Folder."
