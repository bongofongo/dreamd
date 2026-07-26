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

## Open question

Search the **rendered** text or the **raw source**? Rendered is what's on
screen and mirrors the existing precedent in highlight anchoring — quotes
are always matched against rendered DOM text, never raw source (per
`CLAUDE.md`'s anchoring section) — but raw source is likely what a
regex-minded user expects for structural queries (matching across markdown
syntax, etc). Worth settling before writing code, since it changes where the
search runs (DOM walk vs. the source string already held for anchoring).
