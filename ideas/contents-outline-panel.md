# Contents / outline panel

A "contents" view outlining the current markdown file's heading structure,
for quick navigation through a long document. Same idea as what came up in
dialogue as a "navigational preview" — confirmed as one feature, not two;
`ideas/reading-progress-indicator.md` is the separate, distinct concept
(how far through the doc you are, not how to get around it).

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

## Open question

Should it live-update on `file-changed` the same way everything else does,
or only rebuild when the panel is opened? Live-update matches how every
other panel in the app already behaves, so it's the likely default unless
there's a reason to defer it.
