# Vim-style "/" content search

**Status: done — shipped in 6daa0fb, revised in 42abd64 (2026-07-27). Two of
the decisions below were overturned by building it.**

- **Rendered text, not raw source.** The "Decision: raw source" section below is
  wrong and was reversed in the plan (2808e8f). The source was never resident in
  the frontend, and `scanTextNodes` + `nodeIndexAt` already do the offset-to-DOM
  job. So `te**s**t` matches the way the reader sees it and `](` matches
  nothing — the emphasis-splitting blind spot the section flagged as a "known
  limitation" simply doesn't exist. Index built lazily, cached per render.
- **The regex toggle is deleted.** A pattern is searched literally and re-read
  as a regex only where the literal finds nothing. Plain regex was rejected as
  silently wrong in a way the reader can't see (`app.js` would match `appXjs`);
  the fallback can't mislead either way — `.` finds the one literal dot,
  `\bread\b` falls through to what it obviously means, and a lone `(` is just a
  literal, so the invalid-pattern state stopped existing.
- **Nothing searches until Enter.** Painting per keystroke flickered through the
  prefixes of the word being typed and yanked the pane mid-word; the input
  handler and its debounce are gone rather than patched.

Two implementation notes worth carrying forward: matches paint via the CSS
Custom Highlight API rather than `<mark>` wrapping, because a wrap straddling an
existing `mark.hl` leaves two elements sharing one `data-id` and nothing repairs
that — there is no DOM-wrap fallback for old WebKit on purpose. And *declaring*
a `::highlight()` rule cost 27% on the forced layout after a 2MB render with
nothing highlighted, so the rules install into `#find-css` on first `/`; a
session that never searches pays nothing. (Chromium harness, relative signal
only.) That same sheet is emptied while ranges are still registered to force the
stale-paint repaint — a WKWebView invalidation gap that never reproduced in
Chromium and that nothing here can regression-test.

Design doc: `docs/plans/vim-style-content-search.md`.

Pressing `/` opens a basic in-document text search, with regex support,
docked at the bottom of the screen the way vim's command line is.

## Current state

Genuinely new — there's no content search today. `SearchIndex` (`nucleo`,
see `src/search.rs`) is a fuzzy index over file **paths only**; `CLAUDE.md`
already flags content search as v2 scope. The command palette
(`#palette-overlay`) is the closest existing UI, but it's a centered modal
overlay — a different pattern from a bottom-docked search bar.

## What's needed

- A new bottom-docked input, separate from the palette (new markup, not a
  repurposed overlay).
- Match highlighting in the rendered content, plus next/prev navigation
  (vim's `n`/`N` would be the natural fit given the `/` trigger).
- A regex toggle — literal substring by default, opt into JS `RegExp` (with a
  try/catch so an invalid pattern doesn't throw mid-keystroke).

## Decision: raw source — and what that costs

Going with raw source, for speed: the source string is already resident
(it's what `locate`/anchoring reads anyway), so searching it avoids a DOM
walk/serialization on every keystroke. That's real, but it's not free:

- **It doesn't avoid the anchoring problem, it just moves it.** A match
  found in raw source still has to be mapped onto the rendered DOM to
  highlight it and scroll to it — the exact problem `markdown::locate`
  exists to solve for saved highlights, now needed *per search match* on
  every keystroke instead of once per highlight. This is the main hidden
  cost of the speed win.
- **Syntax noise.** Markdown source contains characters invisible in the
  rendered view — a link's raw `[text](url)`, HTML comments, code-fence
  markers, frontmatter. A literal/regex match against source can surface a
  "match" that doesn't correspond to anything the reader can actually see
  on screen, or land at a source offset that doesn't map cleanly to a
  rendered position.
- **The sneaky opposite failure: missed matches.** Text split by inline
  markup — `te**s**t` — reads as "test" once rendered, but as raw source
  it's literally `te**s**t`. A literal or regex search for "test" against
  source would miss it entirely, even though it's sitting right there on
  screen. Rendered-text search wouldn't have this blind spot; raw-source
  search does.

Net: raw source for the speed, accept the DOM-mapping work as real (not
skipped) work, and treat the emphasis-splitting blind spot as a known
limitation to revisit if it turns out to bite in practice.
