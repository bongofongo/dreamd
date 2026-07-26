# Contents / outline panel

A "contents" view outlining the current markdown file's heading structure,
for quick navigation through a long document.

## Current state

Nothing here today. `markdown.rs`'s pulldown-cmark pass emits plain
`<h1>`-`<h6>` with no `id` attributes and no separate heading list — there's
nothing to jump to yet, and no extraction step to build an outline from.

## What's needed

- Assign stable, unique ids to headings during the markdown → HTML pass
  (dedupe repeated heading text — `markdown.rs` already has to reason about
  "repeated heading is periodic" for highlight anchoring, so the same
  disambiguation problem shows up here).
- Produce a heading list (level, text, id) — either as a side channel from
  the Rust render, or by walking the rendered DOM in JS after render, which
  is simpler and needs no Rust change.
- A UI surface: a sidebar tab, a popover, or a panel similar to
  `#stack-panel` — click an entry, scroll/jump to that heading.

## Open questions

- Should it live-update on `file-changed` the same way everything else does,
  or only rebuild when the panel is opened?
- Is this strictly per-document navigation, or does it double as a
  lightweight breadcrumb/progress indicator while reading (fits the
  book-reading direction from the theming pass)?
