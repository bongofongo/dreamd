#!/usr/bin/env bash
# Print the version. src-tauri/Cargo.toml is the single source of truth:
# tauri.conf.json deliberately carries no `version` key so there is one fewer
# place for it to drift, and Tauri falls back to the crate version.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# The first `version =` inside [package] — not a dependency's.
awk '
  /^\[package\]/ { in_pkg = 1; next }
  /^\[/          { in_pkg = 0 }
  in_pkg && /^version[[:space:]]*=/ {
    gsub(/^version[[:space:]]*=[[:space:]]*"|"[[:space:]]*$/, "")
    print; exit
  }
' "$root/src-tauri/Cargo.toml"
