# Idea log — file links, section links, and jump back

Source idea: `ideas/file-and-section-links.md`. Overnight batch, branch
`claude/overnight-ideas-2026-07-27`.

**Decision: split.** The idea bundles two very different things behind one
title. Implemented the link-resolution half; planned the navigation-history
half in `docs/plans/file-and-section-links.md`.

- **Implemented** — path containment on file links, and cross-file
  `other.md#section` jumps. Both are entirely inside `interceptLinks` in
  `ui/app.js`: no Rust, no new IPC command, no keymap entry, nothing outside
  one function and two new helpers next to it.
- **Planned, not built** — jump back / jump forward. It needs a new `Keymap`
  field (which fans out to `config.rs`, the keydown chain, `KEY_ACTIONS`, the
  README and two harness fixtures), a push call at every navigation entry
  point, and answers to two product questions that only the author can give.
  The deciding one: **both keys the idea proposes are already bound.**
  `Ctrl+O` is `toggle_stack` and `Ctrl+I` became `toggle_outline` in commit
  `790a883` yesterday. Picking a default there is a call about the author's own
  muscle memory, not a coding decision, so it is written up rather than guessed
  at.

## What was done

### Path containment (the gap the idea flagged)

The file-link handler normalized the path and checked the extension but never
confirmed the result stayed inside the repo, so a link with enough `../`
segments resolved outside it. It now refuses, with a toast rather than silence.

Factored out as `insideRepo(abs)` and pointed the image handler at it too,
which **fixes a narrower bug in the check the idea cites as the good example**:
`abs.startsWith(repoRoot)` is not containment, because it never reaches a path
separator. With a root of `/w/notes` it also accepts `/w/notes-private/x.md`.
The helper tests "equal to the root, or under root + `/`", tolerates a trailing
slash on `repoRoot`, and keeps the existing behaviour of refusing everything
when no repo is open.

### Cross-file section links

`other.md#a-heading` used to drop the fragment on the floor
(`href.split("#")[0]`). It now splits on the **first** `#` only, opens the
file, and scrolls to the fragment once the render has landed. The ordering
matters and is the reason this awaits: `renderCurrent` sets
`scrollEl.scrollTop` *last*, so scrolling before it resolves would be undone.

The same-document `#anchor` branch and this one now share one
`scrollToFragment(frag)` helper — same id scan, same `decodeURIComponent`, same
`#content` scoping, so the two paths cannot drift. The previous agent's
`querySelectorAll("[id]")` fix (not `querySelector(href)`, which throws on a
valid-id-but-invalid-selector like `#1-intro`) is preserved inside it, and
`scrollToFragment` now returns whether it found anything — unused today, but
it is what a jump-back push would key on.

Fragment ids come from `markdown::Slugger`, so a link written against GitHub's
slugs resolves the same way here. Nothing needed to be reimplemented on the
Rust side.

## Files touched

- `ui/app.js` — `interceptLinks` link branch rewritten; new `insideRepo` and
  `scrollToFragment` helpers; image handler switched to `insideRepo`.
- `README.md` — a "Links" bullet in the reading-experience list.
- `docs/plans/file-and-section-links.md` — the jump-back plan (new).

**`src-tauri/` was NOT touched at all**, so nothing here is missing a compile
check and the build gate was not applicable. No `main.rs` edits.

## Verification

- `node --check ui/app.js` passes.
- `insideRepo` and the first-`#` split were exercised as standalone functions
  in Node against the containment cases (`/w/notes`, `/w/notes/a.md` in;
  `/w/notes-private/x.md`, `/w/other.md`, `/w`, `/w/notesX` out; `/` as root;
  empty root) and the href cases (`a.md`, `a.md#sec`, `a.md#`, `a.md#b#c`,
  `../x/y.md#z`). All as expected.

## Left open

- **Nothing was rendered.** No Tauri/WebKitGTK run and no Chromium (Playwright's
  download is blocked by this container's proxy), so the click paths are
  unexercised in a real webview. The logic is small and was checked in
  isolation, but a link has not actually been clicked.
- **Jump back / jump forward is not built** — `docs/plans/file-and-section-links.md`.
  It opens with the two questions to answer first (keybind defaults; whether
  tree and palette clicks push).
- **A cross-file link to a missing fragment is silent.** If `other.md#nope`
  resolves to a file but no such heading, you land at the top of the document
  with no explanation. Arguably wants a toast; left alone because the
  same-document branch has always behaved that way and consistency beat a
  guess.

perf not run - pending manual check on the author's machine
