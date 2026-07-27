# Idea log: "view mode" keybind (viewer only)

Idea file: `ideas/view-mode-keybind.md`. **Implemented, not planned.** The idea
file already settled the design ("keep it simple for now": a plain
`body.view-mode` CSS toggle, no window-chrome APIs, no dependency on the Mac
title-bar work in `docs/todo2.md`), and the shape is identical to the
`toggle_tree` work that landed in `1895e69` — one new `Keymap` field, one class
flip, six lines of CSS. Nothing new is computed, no IPC is added, no state
persists. That is squarely the "doesn't pose great risk / one of the easier
ones" side of the line.

## Decisions

**Key: `Ctrl+M`.** `Ctrl+O` (stack), `Ctrl+I` (outline) and `Ctrl+B` (tree) were
all taken; `Ctrl+M` is free, mnemonic for "minimal", and unclaimed by the
webview. It does not collide with the macOS menubar (`Cmd+O` / `Cmd+Shift+O`)
because `matchCombo` requires exact modifier equality including `metaKey`.
`Ctrl+\` was the other candidate and was rejected: it needs escaping in a TOML
basic string (`"Ctrl+\\"`), which is a papercut for every hand-edited
`config.toml`. `Ctrl+.` was rejected as too easy to mis-press into `Ctrl+,`
(settings). Rebindable from the settings panel like every other action.

**Interaction with `toggle_tree` (the thing the brief flagged).** `view-mode` is
purely **additive** and never writes another surface's state: it does not set or
clear `nav-collapsed`, and it does not touch `#stack-panel.open` or
`#outline-panel.open`. It only *out-ranks* them in CSS while it is on. So
exiting restores exactly the chrome the user had before entering — a sidebar
they had open comes back open, one they had collapsed with `Ctrl+B` stays
collapsed — and there is no bookkeeping to get out of sync. This is what stops
the "stranded with a hidden sidebar they didn't hide" failure the brief warned
about: nothing was hidden *by state*, only by presentation.

The cost is CSS ordering, which is now load-bearing and commented in place: the
`body.view-mode` block ties on specificity with `body.nav-collapsed #btn-expand`
(both `1,1,1`) and beats `#stack-panel.open` / `#outline-panel.open` (`1,1,0`),
so it must stay *after* all three in the source. It is placed at the end of the
panel rules for that reason.

**Plain toggle, not auto-exit.** As the idea file preferred. The palette,
settings and annotation overlays are `position: fixed` above everything and stay
usable in view mode, so there is no chrome-requiring action that needs to break
out of it.

**`Esc` also exits**, added beyond the idea file. Hiding the titlebar removes
every clickable affordance, so a user who forgot the binding would otherwise be
stuck. `Esc` is the *last* claim on the key: if any overlay or the file menu is
open, that Escape closes it and view mode survives. For the same discoverability
reason, entering raises a one-line toast naming both ways out.

## What was done

- `src-tauri/src/config.rs` — new `Keymap::toggle_view` field + `"Ctrl+M"`
  default, with the rationale for the key on the default.
- `ui/index.html` — one `body.view-mode` CSS block hiding `#titlebar`,
  `#sidebar`, `#btn-expand`, `#outline-panel` and `#stack-panel`, plus the
  zero-width sidebar column. `#btn-expand` matters: without it a
  view-mode-over-collapsed-tree leaves a floating arrow over the reading area.
- `ui/app.js` — matching default in the frontend `keymap` literal;
  `toggleView()` / `exitView()` next to `toggleTree()`; one `matchCombo` branch
  in `wireKeys`; the `Esc` claim check; a `toggle_view` entry in `KEY_ACTIONS`
  so the settings panel renders, rebinds and clash-checks it (`comboClashes`
  iterates `KEY_ACTIONS`, so it picked this up for free).
- `README.md` — usage bullet, the keybind table, and the `[keymap]` sample.
- `perf/harness/lib/fixtures.mjs` and `perf/harness/ui-check.mjs` — the three
  `Keymap::default()` mirrors gained `toggle_view`, and the `every action gets a
  row` assertion went `12` → `13` (12 `KEY_ACTIONS` entries plus the
  `quick_highlight` row).

No titlebar button was added: view mode's own button would be the first thing it
hides.

## Verification

- Build gate: `cargo build --lib` clean; the bin still fails with exactly the 5
  known pre-existing macOS-gating errors. **GATE PASS**.
- `cargo run --example config_check` — 34 passed, 0 failed, so the new field
  survives the layered-TOML merge and write-back path.
- `node --check` on `ui/app.js`, `perf/harness/ui-check.mjs` and
  `perf/harness/lib/fixtures.mjs`.
- **`src-tauri/src/main.rs` was not touched**, so no part of this change escapes
  the compile check.
- `perf/harness/ui-check.mjs` could not be run here (Playwright's Chromium
  download is proxy-blocked). Its fixtures and the row count were updated by
  inspection and need a run on a machine that can fetch Chromium.

perf not run - pending manual check on the author's machine

## Left open

- Not exercised in a real window: the CSS ordering claim above is reasoned from
  specificity, not observed. The one thing to eyeball first is whether
  `#stack-panel.open` and `#outline-panel.open` really do disappear under
  `body.view-mode` — that is the tie the comment is about.
- The toast wording assumes `Esc` is free. It is, today.
- macOS traffic lights are window chrome, not `#titlebar`, so they remain
  visible in view mode. Hiding those is the `docs/todo2.md` work and was
  explicitly out of scope per the idea file; this class is the natural hook for
  it later.
- Not wired to a mouse affordance at all. If that turns out to matter, the
  obvious answer is a hover-reveal strip at the top edge rather than a
  permanently visible button.
