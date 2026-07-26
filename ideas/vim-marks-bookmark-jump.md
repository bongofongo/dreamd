# Vim-style marks: bookmark + jump back

`m{letter}` drops a bookmark at the current scroll position; `` `{letter} ``
jumps back to it. Vim's marks, not a browser-style automatic history.

## Current state

Nothing exists here — no bookmark concept, no stored positions, no chord
key handling at all yet (see the `gg`/`G` open question in
`ideas/jump-top-bottom-keybind.md` — this idea has the same "is `matchCombo`
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
  (`ideas/file-and-section-links.md`): that one is automatic navigation
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
