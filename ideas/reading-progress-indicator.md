# Reading progress indicator

A lightweight indicator of how far through the current document you are —
scroll percentage, or a heading-based "section 3 of 12," rather than a full
navigational surface.

## Current state

Nothing exists here today — no scroll-position tracking, no heading count
exposed to the UI at all (see `ideas/contents-outline-panel.md`, which needs
the same heading-list groundwork).

## Shape of it

- Simplest version: a thin bar or percentage tied to `#content`'s scroll
  position vs. its scrollHeight — pure frontend, no backend change.
- Heading-aware version ("section 3 of 12") depends on the same heading
  extraction the outline panel needs, so it's worth sequencing after that
  work rather than building a second, separate heading-count mechanism.
- Fits the book-reading direction from the theming pass — this is meant to
  feel like a physical bookmark/progress cue, not a developer-tool scrollbar
  minimap (that's the separate `navigational-preview.md` idea).

## Open question

Percentage, heading count, or both? And does it live in the titlebar, as a
thin strip along an edge, or somewhere else?
