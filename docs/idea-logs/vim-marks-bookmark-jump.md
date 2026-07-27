# Vim-style marks: bookmark + jump back

Implemented, not planned. `m{letter}` stores the current file and scroll
position; `'{letter}` returns to it, across files.

## Call: implement

The idea file and `docs/plans/jump-top-bottom-keybind.md` both framed this as
blocked on chord plumbing that does not exist. That is true, but the plan's cost
estimate was for the `gg` shape, and marks are cheaper than it looks:

| | `gg` (planned) | `m{letter}` (built) |
|---|---|---|
| combo grammar | needs a space separator | unchanged |
| rebind UI (`onRecordKey`) | two-state recorder | unchanged — `m` is one key |
| `displayCombo` / `comboClashes` / `comboFromEvent` | all change | unchanged |
| Rust | `Keymap` strings | two `Keymap` strings, no logic |

Because the letter is an *argument* rather than part of the binding, `set_mark`
and `jump_mark` are ordinary single combos and the settings panel needs no idea
marks exist. That deletes the whole surface the plan was worried about and left
one genuinely risky thing: the pending-prefix state machine in the global
keydown handler.

## The open question, answered: global, not per-file

Marks are **global across the repo**, keyed by letter alone. dreamd shows one
document at a time out of a whole repo, so the useful move is "bookmark the spot
in the architecture doc, come back from wherever I've wandered to" — cross-file
by definition. A per-file mark is a bookmark you can only use once you have
already navigated to the thing you were trying to navigate to. Global is also
the smaller build: one flat `Map`, and a jump is `await openFile(path)` plus a
scroll, both of which already existed.

Vim splits a–z (buffer-local) from A–Z (global) because it has a buffer list; a
reader does not, so every letter is global here and case is significant, which
gives 62 marks instead of 26.

## The state machine, and why it is safe

`pendingMark` is the only piece of cross-event input state in the frontend. The
property the design is built around, and which the harness below checks
exhaustively:

> `consumeMarkKey` can consume **only** (a) key repeats while a leader is held,
> and (b) a bare alphanumeric arriving within 1.5 s of a leader key. Every other
> event returns false, unprevented, and falls through to the normal chain.

Concretely: a bare modifier keeps the chord armed and passes through, so
`Shift`+`A` still marks `A`. A modified combo cancels the chord and falls
through, so the palette key still opens the palette. Arrows, `F5`, `Dead` and
`Process` cancel and fall through. A stale leader cancels and falls through, so
an abandoned `m` releases a later `h` to the highlight shortcut. A mark chord
going wrong costs the mark, never the keystroke.

Clearing is centralised rather than scattered: the overlay guard and the
`isEditable` guard in `wireKeys` both `clearMark()` on the way out, which covers
every overlay-open and focus-into-a-field path at once, and every non-modifier
keydown disarms by construction. Plus `Escape`, `window.blur`, and the timeout.

Dispatch order: `Escape` → overlay guard → palette/settings → `isEditable` →
**`consumeMarkKey`** → the existing single-combo chain (with the two `armMark`
lines added next to the file-stepping ones). Sitting below both guards means the
machine only ever sees keystrokes aimed at the document; sitting above the
single-combo chain is what makes `m` then `h` a mark rather than a highlight.

## Files touched

- `ui/app.js` — the marks block next to `jumpTop`/`jumpBottom` (`marks`,
  `pendingMark`, `armMark`, `clearMark`, `consumeMarkKey`, `setMark`,
  `jumpMark`); four small edits in `wireKeys`; two `KEY_ACTIONS` rows.
- `src-tauri/src/config.rs` — `set_mark` / `jump_mark` fields and defaults.
- `perf/harness/ui-check.mjs` — two `KEYMAP` mirrors, and `rows === 17` → `19`.
- `perf/harness/lib/fixtures.mjs` — `KEYMAP` mirror.
- `README.md` — keybind table, prose paragraph, config sample.
- `docs/plans/jump-top-bottom-keybind.md` — status note: rules 1–4 now exist,
  that plan is down to representation + rebind UI. Flags that the marks machine
  deliberately never swallows an unrecognised key, which a fixed sequence like
  `g g` will want to do differently.

## Defaults

`set_mark = "m"`, `jump_mark = "'"`. Bare `m` is a real claim on the keyspace,
but the same claim `quick_highlight`'s bare `h` has made since before keybinds
were configurable, and `m` does nothing else in a reader.

`'` rather than the idea file's `` ` ``: both are correct vim (`'a` jumps to the
line, `` `a `` to the column — a distinction a reader has no use for), and
backtick is a dead key on several international layouts, where it arrives as
`e.key === "Dead"` and could never match. One-line rebind either way. **Flagging
this as a judgement call against the idea file's wording** in case the author
disagrees.

## Verification

- Build gate: `cargo build --lib` clean; bin error count 5, equal to the
  pre-existing macOS-gating baseline. GATE PASS.
- `node --check ui/app.js` clean.
- A scratchpad harness greps `consumeMarkKey`/`setMark`/`jumpMark`/`armMark`/
  `clearMark` **out of `ui/app.js` itself** (not a copy) and drives them: happy
  paths, case-sensitivity, every cancel path, stale leaders, key repeat, and an
  exhaustive sweep of 360 event shapes (18 keys × repeat × 5 modifier sets ×
  stale) asserting the invariant above. All green, 0 violations. The harness is
  scratchpad-only and not committed — there is no unit-test home for frontend
  logic in this repo, and adding one is a bigger decision than this change.

## Left open

- **Not verified in a real webview.** No Chromium (proxy-blocked) and no macOS
  here, so nothing below has been seen working: that `m` and `'` actually reach
  the handler in WKWebView, that the settings panel renders and records the two
  new rows, and that a cross-file jump lands on the right scroll offset after
  `openFile` resolves. The logic is tested; the integration is not.
- **A mark is a pixel offset**, so it drifts if the file changes on disk after
  the mark is dropped. A heading id (the outline work added them) with the
  offset as a fallback would be sturdier and is the obvious follow-up.
- **No armed-state indicator.** Vim is silent here too, but vim has a
  `showcmd` corner. A subtle hint after ~300 ms would help discoverability
  without flashing on every mark.
- **A mark pointing at a deleted file** shows `renderCurrent`'s error pane
  rather than a "that file is gone" toast. The watcher's `file-removed` event
  could prune the map; not wired, to keep the diff tight.
- `ui-check.mjs` has no mark case — it cannot run here, and a chord case needs a
  real browser to be worth anything.

perf not run - pending manual check on the author's machine
