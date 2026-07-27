# Keybind: jump to top/bottom (gg/G)

**Status: done — shipped in 1f724a2 (2026-07-26), with the open question
resolved against `gg`.** The bindings are `Home` / `End`, not `gg` / `G`:
single keys with the same effect, so the chord-recording plumbing the `gg`
chord would have needed was never built. `jump_top` / `jump_bottom` are
ordinary rebindable `Keymap` entries. Design doc:
`docs/plans/jump-top-bottom-keybind.md`.

Vim's `gg` (top) and `G` (bottom) for the document viewer.

## Current state

No such shortcut exists — plain scroll only.

## What's needed

- Cheapest idea in this batch: `#content.scrollTo({ top: 0 })` / `scrollTo`
  bottom, no backend involvement at all.
- `G` is a single keystroke and already fits `matchCombo`'s handling. `gg`
  is a two-key chord (press `g` twice) — check whether the existing keydown
  dispatch (`ui/app.js:819+`) and `matchCombo`/combo-recording UI support
  chorded sequences today, or only single combos with modifiers; if it's the
  latter, `gg` needs a small addition (a short-lived "last key was g, and
  recently" check) rather than a plain `Keymap` entry.
- Settings-panel entries for both, same as the other keybind ideas — though
  a two-key chord may not fit the existing single-combo rebind UI without
  that same small addition.

## Open question

Is `gg`/`G` worth the chord-handling wrinkle, or is `Home`/`End` (single
keys, same jump-to-top/bottom effect) close enough to the vim feel without
needing new chord-recording plumbing?
