# Plan: the heading-aware half of the reading progress indicator

Companion to the shipped half of `ideas/reading-progress-indicator.md`. The
scroll-position half landed: a `#progress-pct` readout in the titlebar and a
`#progress-rail` hairline at the foot of the reading pane, both driven by one
rAF-coalesced `scrollTop` read (see the `reading progress` section of
`ui/app.js`). This document covers the part that was deliberately *not* built —
the "section 3 of 12" variant the idea file asks about — so that whoever wants
it has a design rather than a blank page.

**Read the recommendation before building it.** It is not "next up"; it is
"here is what it would take, and here is why the shipped version answers the
question better."

## Recommendation: don't build it as stated

Three reasons, in order of weight.

1. **A section counter measures structure, not position, and the idea file asks
   for position.** "How far am I" is a distance question. On the documents
   dreamd is actually pointed at — a repo's `docs/`, a session log, a design
   note — heading sizes are wildly uneven: `## Architecture` in this repo's own
   `CLAUDE.md` is eight screens and `## Docs` is half of one. "Section 3 of 12"
   in a document like that is off by tens of percent as a position estimate,
   and it is *confidently* off, which is worse than the honest 41% the rail
   already shows.

2. **It overlaps the surface that just shipped.** `#outline-panel` (commit
   `790a883`) lists every heading, indented by level, one click from each. A
   reader who wants to know which section they are in has a panel that tells
   them, with the surrounding structure for free. A counter in the titlebar is
   the same information with the context stripped out.

3. **It needs a scroll-spy, which is the one thing this feature was built to
   avoid.** The shipped indicator reads `scrollTop` once per frame against a
   cached extent, and writes a composited `transform`. "Which heading am I in"
   needs per-heading geometry — either an `IntersectionObserver` over every
   `h1…h6`, or a binary search over cached `offsetTop`s. Both are affordable;
   neither is free, and `ui/index.html`'s `content-visibility` note is a
   standing reminder that scroll cost in this app has already been measured and
   traded once.

If it is built anyway, the shape below is the cheap way in.

## Shape, if it is built

### Where the heading list comes from

Nowhere new. `markdown::Slugger` already gives every heading an `id` in the
render pass, and `buildOutline()` in `ui/app.js` already walks
`contentEl.querySelectorAll("h1, h2, h3, h4, h5, h6")`. **No Rust change and no
new IPC.** The counter and the outline should share one cached
`NodeList`/array, refreshed on the same `refreshOutline()` beat, or they will
drift on the file-changed path.

Two decisions to make before the first line of code:

- **Which levels count.** `h1…h6` makes "12 sections" mean "12 headings",
  which is not what a reader means by section. Counting `h1`+`h2` only is
  closer, but breaks on a document that is all `h3`. The least-bad rule is
  *the shallowest level present in this document* — one pass over the list,
  `Math.min(level)`, and count only that level. It is stable per render and
  needs no configuration.
- **What "before the first heading" is.** A document's preamble belongs to no
  section. Show nothing there rather than "section 0" or "section 1".

### Tracking the current section

`IntersectionObserver`, not a scroll handler. One observer over the counted
headings with

```js
rootMargin: "0px 0px -85% 0px", threshold: 0
```

against `root: scrollEl`, which turns the top ~15% of the pane into the
"cursor". The callback fires only when a heading crosses that band — a few
times per document, not once per frame — and maintains the index by taking the
last heading whose entry is intersecting, falling back to the last one that
left through the top.

Two traps that will bite:

- **A heading taller than the band, or two headings inside it at once,** makes
  "the intersecting one" ambiguous. Keep the *last* one in document order; that
  matches what the reader's eye is on.
- **The observer must be torn down and rebuilt on every render**, because
  `contentEl.innerHTML` replaces the nodes. `refreshOutline()` is the existing
  hook; `disconnect()` first, unconditionally, or a re-render leaks an observer
  per save and the file-changed path is the hot one.

The alternative — cache `offsetTop` per heading at render time and binary-search
it inside the existing rAF draw — costs one array build per render and zero
observers, and folds into `drawProgress` with no second mechanism. It is the
better fit for this codebase if the numbers ever say the observer is not worth
its wiring, but it needs invalidating on every `ResizeObserver` fire (images,
window width, content width), which `measureProgress` is already the place for.

### Where it would show

Not a second titlebar element. `#progress-pct` is 42px of fixed-width chrome
precisely so the digits cannot reflow the bar; "Section 3 of 12" is variable
width and would undo that. If both are wanted, the counter belongs in the
`#outline-panel` header (`<h3>Contents</h3>`), where the structure it describes
is already on screen, or as a `data-tip` on the percentage readout — which
costs nothing until hovered and needs no live tracking at all, since the
tooltip can compute the section on `showTip`.

**The tooltip route is the one to take.** It answers "which section am I in" on
demand, needs no `IntersectionObserver`, no per-frame work, no teardown
discipline, and no new pixels. `wireTooltips()` already delegates on
`[data-tip]` and `showTip()` already reads `el.dataset.tip` at display time —
so the whole feature is: on show, find the last counted heading whose
`getBoundingClientRect().top` is above the band, and render
`"Section N of M — <heading text>"`. That is one `getBoundingClientRect` loop,
once, on a deliberate hover.

## If it ships, measure it

`perf/harness/`'s scroll scenario is the one that matters, and the comparison
is against the rail-only build, not against `main` before either. Chromium
numbers only — relative regression signal, per `perf/README.md`. The specific
thing to watch is main-thread time per wheel tick, which is the metric that
already decided the `content-visibility` question in `ui/index.html`.
