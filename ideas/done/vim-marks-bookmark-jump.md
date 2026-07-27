# Vim-style marks: bookmark + jump back

**Status: done — shipped in 583a467, then deliberately scoped down in 587094e
(2026-07-26/27).** Shipped as designed first: `m{letter}` / `'{letter}`, global
across the repo (not per file — the open question below), in memory only per
tenet 2, with a pending-prefix state machine in the keydown handler.

**It is now one mark with no letter argument.** The 26-letter version was never
broken — `ma`, `]`, `'a` restored file and offset exactly — but `m` alone did
nothing observable, so the feature *read* as dead. One mark makes both keys
ordinary single combos that confirm immediately, and deleted the largest piece
of input state in the frontend: `pendingMark`, `MARK_TIMEOUT_MS`,
`armMark`/`clearMark`/`consumeMarkKey`, the repeat guard, the Escape branch,
four `clearMark()` calls and a blur listener. That machinery existed to serve a
second bookmark nobody had asked for.

Marks share `{ path, top }` and `restoreFrame` with the jump history from
`ideas/done/file-and-section-links.md`, as anticipated. If the 26-letter version is
ever wanted back, the argument to answer first is what makes `m` alone visibly
do something.

`m{letter}` drops a bookmark at the current scroll position; `` `{letter} ``
jumps back to it. Vim's marks, not a browser-style automatic history.

## Current state

Nothing exists here — no bookmark concept, no stored positions, no chord
key handling at all yet (see the `gg`/`G` open question in
`ideas/done/jump-top-bottom-keybind.md` — this idea has the same "is `matchCombo`
built for chords" question, and more of them: `m{letter}` is itself a
two-key chord with a variable second key, not a fixed combo).

## What's needed

- A small stored-position stack/map: file + scroll offset per mark letter.
  In-memory only, dies with the process — consistent with tenet 2 (no
  session state persists) unless you specifically want marks to survive a
  restart, which would be a deliberate exception like the config/theme one.
- Chord input handling distinct from the rest of the keymap: `m` then any
  letter sets that mark, `` ` `` then any letter jumps to it. This doesn't
  fit the current single-combo `matchCombo`/settings-panel rebind pattern
  (fixed key + modifiers) — it's a genuinely different input shape
  (leader key + arbitrary following key), so it likely needs its own small
  piece of state in the keydown handler rather than a `Keymap` entry per
  letter.
- Relationship to the file/section-link "jump back" idea
  (`ideas/done/file-and-section-links.md`): that one is automatic navigation
  history (jump back to where you *were*, un-asked); this one is explicit,
  user-placed bookmarks. They could share the same underlying
  "stack of {file, scroll} positions" plumbing if it's useful to build once,
  but the triggers and mental model are different — worth keeping them as
  two ideas rather than merging.

## Open question

Scoped per-file, or global across the whole repo (jump to a mark in a
different file, the way vim marks work across buffers)? Global is more
useful but means marks need to survive `openFile` swapping out the
current document.
