# Idea log: jump to top/bottom (gg/G)

Idea file: `ideas/jump-top-bottom-keybind.md`. **Split: implemented + planned.**
The jump landed as two ordinary keybinds; the `gg` chord did not, and is written
up in `docs/plans/jump-top-bottom-keybind.md` instead.

## The central question, answered

**Chord support does not exist, and no part of the current tree is a step toward
it.** `matchCombo` (`ui/app.js:1272`) is entirely stateless: it splits a combo on
`+`, requires exact equality on all four modifiers, and compares one `e.key`.
Nothing in the frontend remembers the previous keystroke. The rebind UI matches:
`onRecordKey` captures exactly one `keydown` and `comboFromEvent` encodes it as
`Ctrl+Shift+X`; `comboClashes` compares whole strings; `displayCombo` splits on
`+`. The global handler in `wireKeys` is a flat if-chain of single `matchCombo`
calls.

**For the `ideas/vim-marks-bookmark-jump.md` agent: you start from nothing.** No
pending-key state, no leader mechanism, no capture-next-keystroke path exists
after this commit. `docs/plans/jump-top-bottom-keybind.md` has a section, "Two
shapes, one prefix", written specifically for you: `gg` (fixed second key) and
`m{letter}` (arbitrary second key, which *is* the argument) share a pending-
prefix state machine but not a representation, and the recommendation there is
to build the arbitrary-key `captureNext` primitive first because the fixed
sequence expresses on top of it and not the reverse. The plan's rules 1–4
(timeout, clear-before-action, clear-on-overlay-open, clear-on-blur) apply
unchanged to marks; its grammar and rebind-UI sections do not.

## What was implemented

Two new `Keymap` actions, `jump_top` and `jump_bottom`, defaulting to `Home` and
`End`, following the `toggle_outline` / `toggle_tree` / `toggle_view` pattern
exactly: a field on `Keymap`, a default, one `matchCombo` line in `wireKeys`, one
`KEY_ACTIONS` entry each, mirrors in the harness.

The action itself is `scrollEl.scrollTo({ top: 0 })` /
`scrollTo({ top: scrollEl.scrollHeight })`. No IPC, no backend involvement, no
state — the cheapest thing in the batch, as the idea file predicted.

## Decisions

**`Home` / `End` as defaults, not `gg` / `G`.** These are single keys that
`matchCombo` already handles, they are unbound in the app today, and they cost no
letter keyspace. They are also genuinely load-bearing rather than redundant with
the browser: `#content-scroll` is the scroller, not the window, so the native
`Home`/`End` do nothing unless focus happens to be inside that div.

**No hardcoded bare-letter alias for `G`.** `Shift+G` is one rebind away in the
settings panel for anyone who wants it, and an unconditional bare-`G` alias could
not be turned off. The project's own precedent points this way: `quick_highlight`
exists precisely because bare-letter shortcuts are opt-in here, not default. The
plan doc proposes a `vim_jumps` flag in the same shape if the chord work lands.

**Instant scroll, not smooth.** Everything else that moves this pane
(`scrollIntoView`, restoring `scrollTop` after a re-render at `app.js:415`) is
instant, and a smooth animation over a long document is the one place scrolling
in this app can jank.

**The chord was planned, not built, because it is a different risk class.** The
cost is not the `gg` matcher — it is that the change reaches `matchCombo` (8 call
sites, two inside overlay-local handlers), the recording UI, the clash check, the
combo renderer, and the grammar that `config.toml` round-trips. The other three
keybind ideas in this batch were "one field, one if-line". This is not, and the
user's criteria put it on the plan side. `Home`/`End` deliver the user-facing
behaviour at the cheap risk level; the plan carries the rest.

## Files touched

- `src-tauri/src/config.rs` — `Keymap::jump_top` / `jump_bottom` + defaults.
- `ui/app.js` — fallback keymap literal; `jumpTop()` / `jumpBottom()` next to
  `exitView()`; two dispatch lines in `wireKeys` after `toggle_stack`; two
  `KEY_ACTIONS` entries.
- `perf/harness/lib/fixtures.mjs`, `perf/harness/ui-check.mjs` — the two
  `Keymap::default()` mirrors in `ui-check.mjs` and the one in `fixtures.mjs`;
  the "every action gets a row" assertion bumped 13 → 15.
- `README.md` — the `[keymap]` config block.
- `docs/plans/jump-top-bottom-keybind.md` — new.

**`src-tauri/src/main.rs` was not touched.** No new `#[tauri::command]` was
needed: `default_keymap` already serializes `Keymap::default()` wholesale, so the
new fields flow through the existing command untouched.

## Verification

- The overnight build gate printed **GATE PASS** — `cargo build --lib` clean, and
  the bin still fails with exactly the 5 pre-existing macOS-gating errors and no
  new ones.
- `cargo run --example config_check` — 34 passed, 0 failed. This is the one that
  matters here: it exercises the layered-TOML merge and write-back over the
  widened `Keymap`.
- `node --check ui/app.js` — clean.
- `perf/harness/ui-check.mjs` could **not** be run in this container (the
  Playwright Chromium download is proxy-blocked). The `rows === 15` bump and the
  three keymap mirrors were updated by hand and reviewed by eye.

perf not run - pending manual check on the author's machine

## Left open

- `gg` / `G`, in full, in `docs/plans/jump-top-bottom-keybind.md`.
- The spacebar trap named in that plan is latent *today*: `comboFromEvent` would
  encode a spacebar binding as `" "`, which nothing currently binds, but a
  space-separated sequence grammar would turn it into a real ambiguity. Worth
  fixing (`"Space"` in the encoder, decoded in `matchCombo`) whether or not the
  chord work ever happens.
- Unverified by machine here: that `Home`/`End` in the settings panel's rebind
  recorder round-trip cleanly. `comboFromEvent` pushes `e.key` verbatim, giving
  `"Home"`, which `matchCombo` lowercases to `"home"` and compares against a
  lowercased `e.key` — correct by inspection, but only inspection.
