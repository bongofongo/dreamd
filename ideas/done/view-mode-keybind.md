# Keybind: "view mode" (viewer only)

**Status: done — shipped in 308bb46, fixed in 587094e (2026-07-26/27).** A
plain `body.view-mode` toggle on `toggle_view`, default `Ctrl+M`, as the
decision below scoped it — no window-chrome API calls.

The fix is worth keeping: view mode originally hid the sidebar with
`display: none`, which takes it out of grid placement rather than giving it a
zero-width track the way `nav-collapsed` does. With `grid-template-columns:
0 1fr` still declared, `#main-wrap` became the first grid item and landed in the
`0` track — the feature painted an empty window. It now declares one track in
view mode. Note that all 41 harness assertions passed while it drew nothing;
the replacement checks measure width on purpose.

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
  pattern as `ideas/done/hide-file-tree-keybind.md`.
- Decide whether it's a plain toggle (press again to restore) or restores
  automatically on any chrome-requiring action (opening the palette, stack,
  etc.) — plain toggle is simpler and more predictable.

## Decision: keep it simple for now

Confirmed — this is its own simple, cross-platform mode: a plain
`body.view-mode` CSS toggle via keybind, no window-chrome API calls, no
dependency on the Mac title-bar/traffic-light work in `docs/todo2.md`. That
Mac-specific piece can extend or reuse this later, but isn't a prerequisite
and shouldn't gate this from shipping now.
