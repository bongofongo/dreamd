# Vim-style "/" content search

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
