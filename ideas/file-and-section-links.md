# File links, section links, and jump back

Clicking a link to another file in the repo tree navigates there; clicking a
link to a section/heading (in the same doc or another) jumps to that scroll
position; both directions get a "jump back."

## Current state — more exists than expected, and one part is quietly broken

- **Cross-file relative links already work.** `interceptLinks()`
  (`ui/app.js:252-274`) already intercepts relative `.md` links and calls
  `openFile(target)` — this part of the idea is mostly done.
- **In-doc section links are wired up but don't actually work yet.**
  `href="#..."` clicks already call `querySelector(href)` to scroll to the
  target (`ui/app.js:258-263`) — but headings carry no `id` attribute at all
  today (confirmed in `ideas/contents-outline-panel.md`), so a link to
  `#some-heading` currently finds nothing and silently no-ops. This is the
  same missing heading-id work the outline panel needs — one piece of work,
  two features depending on it.
- **Jump back doesn't exist in any form.** No navigation history/stack is
  kept anywhere — `openFile` just swaps the current document with no memory
  of where you came from.
- **Worth fixing regardless of new scope:** the image-src handler
  explicitly checks the resolved path stays inside `repoRoot`
  (`ui/app.js:283`); the file-link handler (`ui/app.js:267-273`) does not —
  it normalizes the path and checks the extension, but never confirms the
  result is still inside the repo root the way images do. A relative link
  with enough `../` segments could point outside the repo. Small, existing
  gap, independent of anything below.

## What's needed for "jump back"

- A navigation history: a stack of `{file, scrollPos}` (or `{file, anchor}`)
  pushed every time a link-triggered navigation happens (not on manual tree
  clicks, unless you want those remembered too — worth deciding).
- A keybind to pop it — vim's own convention for this exact feature is
  Ctrl-O (jump back) / Ctrl-I (jump forward) through the jumplist, which
  maps directly onto "what this feature already is" rather than needing an
  invented shortcut.
- Relationship to `ideas/vim-marks-bookmark-jump.md`: that's explicit,
  user-placed bookmarks; this is automatic, per-navigation history. Could
  share the same `{file, scroll}` stack shape, but the two triggers (every
  link click vs. an explicit `m{letter}`) are different enough to keep as
  separate ideas rather than one feature.

## Open question

Does a plain scroll-to-heading (no link involved — just browsing) also push
onto the jump-back stack, or only actual link clicks? Vim's jumplist
includes some non-link motions too (search jumps, `G`, etc.) — worth deciding
how far "jump back" should reach before building it.
