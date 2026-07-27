# Keybind: hide the file tree

**Status: done — shipped in 1895e69 (2026-07-26).** `toggle_tree` in `Keymap`,
default `Ctrl+B` (the answer to the open question below), rebindable from the
settings panel like every other shortcut.

A keyboard shortcut that hides/shows the sidebar file tree.

## Current state

The toggle itself already exists as buttons only: `#btn-collapse` /
`#btn-expand` flip `body.nav-collapsed` (`ui/app.js:751-752`). No keybind
drives it yet.

## What's needed

- A new `Keymap` field (`src-tauri/src/config.rs`, the `Keymap` struct +
  `Default` impl) — call it `toggle_tree` or similar.
- An entry in the settings-panel actions list in `ui/app.js` (the array
  around line 974, e.g. `{ id: "toggle_stack", label: "Toggle stack panel" }`)
  so it shows up and is user-rebindable like every other shortcut, not
  hardcoded.
- One `matchCombo` branch in the global keydown handler (`ui/app.js:819+`)
  that toggles `nav-collapsed` the same way the buttons do.

## Open question

Default key? Sidebar-toggle in similar apps is often Cmd/Ctrl+B — fits or
want something else?
