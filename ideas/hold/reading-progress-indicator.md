# Reading progress indicator

**Status: on hold — built in 5e2cad1, removed in 6daa0fb (2026-07-26/27), at the
author's request. No fault found.** It shipped as a percentage in the top bar
plus a rail at the foot, and was withdrawn on taste, not on a bug — so this is
parked rather than done or broken. The plan and the commits stay as history —
see `docs/plans/reading-progress-indicator.md`. If it comes back, the open
question below is the one that decided nothing the first time and would need a
firmer answer.

Note the "Current state" section is stale in one respect: heading extraction now
exists (`markdown::Slugger` + heading ids, 790a883), so the heading-aware
"section 3 of 12" variant no longer needs groundwork built for it.

A lightweight indicator of how far through the current document you are —
scroll percentage, or a heading-based "section 3 of 12," rather than a full
navigational surface.

## Current state

Nothing exists here today — no scroll-position tracking, no heading count
exposed to the UI at all (see `ideas/done/contents-outline-panel.md`, which needs
the same heading-list groundwork).

## Shape of it

- Simplest version: a thin bar or percentage tied to `#content`'s scroll
  position vs. its scrollHeight — pure frontend, no backend change.
- Heading-aware version ("section 3 of 12") depends on the same heading
  extraction the outline panel needs, so it's worth sequencing after that
  work rather than building a second, separate heading-count mechanism.
- Fits the book-reading direction from the theming pass — this is meant to
  feel like a physical bookmark/progress cue, not a navigation tool. Getting
  *around* the document is `ideas/done/contents-outline-panel.md`'s job; this one
  only answers "how far am I."

## Open question

Percentage, heading count, or both? And does it live in the titlebar, as a
thin strip along an edge, or somewhere else?
