#!/bin/sh
#
# dreamd installer.
#
#   curl -fsSL https://raw.githubusercontent.com/bongofongo/dreamd/main/packaging/install.sh | sh
#
# macOS: installs dreamd.app into /Applications and symlinks the CLI onto your
# PATH. They are the same binary — dreamd.app/Contents/MacOS/dreamd is both the
# window and the command line, so the two can never drift apart.
#
# Linux: installs the single `dreamd` binary — likewise both the window and the
# CLI — into ~/.local/bin, plus the desktop entry and icons into ~/.local/share.
# Never root, never /usr: the .deb and the AppImage are the system-wide channels.
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
  # -L on macOS (the CLI is a symlink into the .app), -f on Linux (it is the
  # binary itself). Checking both covers either install without guessing which
  # platform put it there.
  for d in /usr/local/bin "$HOME/.local/bin" /opt/homebrew/bin; do
    { [ -L "$d/dreamd" ] || [ -f "$d/dreamd" ]; } && rm -f "$d/dreamd" && say "removed $d/dreamd"
  done
  xdg="${XDG_DATA_HOME:-$HOME/.local/share}"
  for f in "$xdg"/applications/dreamd.desktop "$xdg"/icons/hicolor/*/apps/dreamd.png; do
    [ -f "$f" ] && rm -f "$f" && say "removed $f"
  done
  say "dreamd removed. Your config in ~/.config/dreamd was left alone."
  exit 0
fi

# ---- platform --------------------------------------------------------------
#
# Everything below branches on `os`, the way build.sh branches on the triple:
# one variable, set once, rather than a `uname` test at each step.

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Darwin)
    # uname -m lies under Rosetta: a translated shell on an Apple Silicon Mac
    # reports x86_64, and we would install the slower build on hardware that can
    # run native.
    if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
      arch="arm64"
    fi
    case "$arch" in
      arm64)  target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *)      die "unsupported architecture: $arch" ;;
    esac
    ext="zip"
    command -v ditto >/dev/null 2>&1 || die "ditto is required"
    ;;
  Linux)
    case "$arch" in
      x86_64) target="x86_64-unknown-linux-gnu" ;;
      *)      die "no prebuilt Linux binary for $arch — build from source: https://github.com/$REPO" ;;
    esac
    # The .tar.gz is the bare binary. The AppImage would need FUSE and the .deb
    # would need dpkg; a tarball needs neither, which is what makes this the
    # right artifact for a curl | sh.
    ext="tar.gz"
    command -v tar >/dev/null 2>&1 || die "tar is required"
    ;;
  *)
    die "unsupported platform: $os — build from source: https://github.com/$REPO"
    ;;
esac

command -v curl >/dev/null 2>&1 || die "curl is required"

# macOS ships perl's `shasum`; Linux ships coreutils' `sha256sum` and often not
# `shasum` at all. Same output shape, so one wrapper covers both.
if command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1"; }
elif command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1"; }
else
  die "neither shasum nor sha256sum is available — cannot verify the download"
fi

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

curl -fsSL "$base/$name.$ext"        -o "$tmp/$name.$ext"    || die "download failed: $base/$name.$ext"
curl -fsSL "$base/$name.$ext.sha256" -o "$tmp/$name.sha256"  || die "checksum download failed"

want="$(tr -d '[:space:]' < "$tmp/$name.sha256")"
got="$(sha256 "$tmp/$name.$ext" | awk '{print $1}')"
[ "$want" = "$got" ] || die "checksum mismatch — expected $want, got $got"
say "    checksum ok"

# ---- install ---------------------------------------------------------------

if [ "$os" = "Darwin" ]; then
  # ditto, matching how it was packed: a .app's code signature lives partly in
  # extended attributes, and unzip(1) drops them.
  ditto -x -k "$tmp/$name.$ext" "$tmp/x" || die "could not extract the archive"
  [ -d "$tmp/x/$APP_NAME" ] || die "archive did not contain $APP_NAME"

  # Cheap, and it turns a corrupted or tampered download into a clear error here
  # rather than a Gatekeeper mystery two minutes from now.
  if ! codesign --verify --deep --strict "$tmp/x/$APP_NAME" 2>/dev/null; then
    say "    warning: signature could not be verified (unsigned build?)"
  fi

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

  exe="$appdir/$APP_NAME/Contents/MacOS/dreamd"
else
  # An FHS tree: usr/bin/dreamd plus the .desktop entry and hicolor icons the
  # Tauri bundler generated for the .deb.
  tar -xzf "$tmp/$name.$ext" -C "$tmp" || die "could not extract the archive"
  [ -f "$tmp/usr/bin/dreamd" ] || die "archive did not contain usr/bin/dreamd"
  chmod +x "$tmp/usr/bin/dreamd"
  exe=""
fi

# ---- put it on PATH --------------------------------------------------------
#
# On macOS this symlinks into the .app so the CLI and the window are provably
# the same binary. On Linux there is no bundle to point at, so the binary is
# copied here and this *is* the install.
#
# $HOME/.local/bin comes first on Linux: the systemd/XDG default, already on
# PATH in most shells, and writable without sudo.

if [ "$os" = "Darwin" ]; then
  bindirs="/usr/local/bin $HOME/.local/bin /opt/homebrew/bin"
else
  bindirs="$HOME/.local/bin /usr/local/bin"
fi

bindir=""
for d in $bindirs; do
  if [ "$os" != "Darwin" ] && [ "$d" = "$HOME/.local/bin" ]; then mkdir -p "$d" 2>/dev/null || true; fi
  if [ -d "$d" ] && [ -w "$d" ]; then bindir="$d"; break; fi
done

if [ -n "$bindir" ]; then
  if [ -n "$exe" ]; then
    ln -sf "$exe" "$bindir/dreamd"
    say "    linked $bindir/dreamd"
  else
    install -m 755 "$tmp/usr/bin/dreamd" "$bindir/dreamd" 2>/dev/null \
      || { cp "$tmp/usr/bin/dreamd" "$bindir/dreamd" && chmod 755 "$bindir/dreamd"; } \
      || die "could not install to $bindir"
    say "    installed $bindir/dreamd"
  fi
  case ":$PATH:" in
    *":$bindir:"*) ;;
    *) say "    note: $bindir is not on your PATH — add: export PATH=\"$bindir:\$PATH\"" ;;
  esac
else
  # Never escalate silently inside a curl | sh. Print the command and let the
  # user run it with their eyes open.
  say ""
  say "    No writable bin directory found. To put dreamd on your PATH, run:"
  if [ -n "$exe" ]; then
    say "        sudo ln -sf '$exe' /usr/local/bin/dreamd"
  else
    say "        sudo install -m 755 '$tmp/usr/bin/dreamd' /usr/local/bin/dreamd"
    say "    (that temp directory is deleted when this script exits — re-run with a writable ~/.local/bin instead)"
  fi
fi

# ---- desktop entry (Linux) -------------------------------------------------
#
# Into ~/.local/share, never /usr/share: this script must not need root, and the
# XDG data dirs are searched in that order anyway. Best-effort — a machine with
# no desktop session (a cloud box, a container) still gets a working CLI, which
# is why nothing here is fatal.

if [ "$os" != "Darwin" ] && [ -n "$bindir" ]; then
  xdg="${XDG_DATA_HOME:-$HOME/.local/share}"
  if mkdir -p "$xdg/applications" 2>/dev/null && [ -d "$tmp/usr/share/applications" ]; then
    cp "$tmp"/usr/share/applications/*.desktop "$xdg/applications/" 2>/dev/null \
      && say "    installed desktop entry in $xdg/applications"
    if [ -d "$tmp/usr/share/icons" ]; then
      cp -R "$tmp/usr/share/icons/." "$xdg/icons/" 2>/dev/null || true
    fi
    # Without this the entry can take until the next login to appear.
    command -v update-desktop-database >/dev/null 2>&1 \
      && update-desktop-database "$xdg/applications" >/dev/null 2>&1 || true
  fi
fi

say ""
if [ "$os" = "Darwin" ]; then
  say "Done. Run 'dreamd' inside a repo, or open the app and use File > Open Folder."
else
  say "Done. Run 'dreamd' inside a repo, or launch it and use File > Open Folder."
fi
