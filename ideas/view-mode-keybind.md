# Keybind: "view mode" (viewer only)

A keybind that hides everything except the markdown viewer — sidebar, stack
panel, title bar/toolbar — for distraction-free reading.

## Current state

No such combined mode exists. The three chrome pieces toggle independently
today: sidebar via `nav-collapsed`, `#stack-panel` via its own `open` class,
`#titlebar` has no hide state at all.

## What's needed

- A single `body`-level class (e.g. `body.view-mode`) whose CSS hides
  sidebar, stack panel, and titlebar together, rather than three separate
  toggles left in whatever state they were in.
- A `Keymap` entry + settings-panel action entry + keydown branch, same
  pattern as `ideas/hide-file-tree-keybind.md`.
- Decide whether it's a plain toggle (press again to restore) or restores
  automatically on any chrome-requiring action (opening the palette, stack,
  etc.) — plain toggle is simpler and more predictable.

## Decision: keep it simple for now

Confirmed — this is its own simple, cross-platform mode: a plain
`body.view-mode` CSS toggle via keybind, no window-chrome API calls, no
dependency on the Mac title-bar/traffic-light work in `docs/todo2.md`. That
Mac-specific piece can extend or reuse this later, but isn't a prerequisite
and shouldn't gate this from shipping now.
