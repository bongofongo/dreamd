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

## Relationship to the Mac title-bar work

`docs/todo2.md` already has a Mac-only idea to hide the title bar + traffic
lights as a packaged toggle. Worth deciding up front whether "view mode" here
*is* that toggle (cross-platform, content-only, no window-chrome API calls),
or a separate, simpler mode both platforms get immediately, with the Mac
window-chrome piece layered on later as its own thing. Leaning toward: build
this one first as pure CSS/keybind (cheap, ships on both platforms today),
let the Mac-specific window-transparency work extend it later rather than
gating on it.
