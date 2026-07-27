# Idea log: keybind to hide the file tree

Idea file: `ideas/hide-file-tree-keybind.md`. Implemented, not planned — the
toggle already existed as `body.nav-collapsed` driven by two buttons, so this is
one new `Keymap` field wired to a class flip. Nothing new is computed, no IPC is
added, and no state persists.

## Decision on the open question

Default is **`Ctrl+B`**, the answer the idea file suggested. It was left free on
purpose by the outline-panel work (`790a883`), it is what every editor spells
sidebar-toggle as, and nothing else in the app or the macOS menubar claims it —
the menu accelerators are `Cmd+O` / `Cmd+Shift+O`, and `matchCombo` requires
exact modifier equality including `metaKey`, so a Cmd chord could not reach it
anyway. Rebindable from the settings panel like every other action.

## What was done

- `src-tauri/src/config.rs` — new `Keymap::toggle_tree` field + `"Ctrl+B"`
  default. The comment on `toggle_outline` that reserved `Ctrl+B` for this idea
  is now stale, so it was rewritten.
- `ui/app.js` — matching default in the frontend `keymap` literal (overwritten
  from Rust at startup, but it is kept complete); a `toggleTree()` next to
  `toggleStack()`; one `matchCombo` branch in `wireKeys`; a `toggle_tree` entry
  in `KEY_ACTIONS` so the settings panel renders and rebinds it.
- `ui/index.html` — `data-tip-key="toggle_tree"` on `#btn-collapse` and
  `#btn-expand`, so both buttons' tooltips show the current binding the way the
  outline and stack buttons do.
- `README.md` — usage bullet, the keybind table, and the `[keymap]` sample.
- `perf/harness/lib/fixtures.mjs` and `perf/harness/ui-check.mjs` — the three
  `Keymap::default()` mirrors gained `toggle_tree`, and the `every action gets a
  row` assertion went `11` → `12`.

`toggleTree` deliberately does not touch the two buttons' handlers: each button
is only visible in the state it acts on, so they stay one-way and the keybind is
the only caller that has to flip both directions.

## Verification

- Build gate: `cargo build --lib` clean; the bin still fails with exactly the 5
  known pre-existing macOS-gating errors. **GATE PASS**.
- `cargo run --example config_check` — 34 passed, 0 failed, so the new field
  survives the layered-TOML merge and write-back path.
- `node --check` on both edited JS files.
- **`main.rs` was not touched**, so nothing in this change is outside the
  compile check.
- `perf/harness/ui-check.mjs` could not be run here (Playwright's Chromium
  download is proxy-blocked). Its fixtures and the row count were updated by
  inspection and need a run on a machine that can fetch Chromium.

perf not run - pending manual check on the author's machine

## Left open

- Not exercised in a real window: the keybind, the tooltips now showing `⌃B`,
  and the interaction with a single-file launch (which starts collapsed — the
  keybind is the fastest way back to the tree there, but that was not observed).
- With no repo (`.app` double-clicked from Finder, `hasRepo === false`) the
  keybind will happily expand an empty sidebar, exactly as `#btn-expand`
  already does. Left alone rather than special-cased.
