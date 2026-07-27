# Idea log — reading progress indicator

Source idea: `ideas/reading-progress-indicator.md`. Overnight batch, branch
`claude/overnight-ideas-2026-07-27`.

**Decision: split — the scroll-position half implemented, the heading-aware
half planned.** The idea file itself splits along that line ("simplest
version… pure frontend, no backend change" vs. "heading-aware version…
depends on the same heading extraction"), and the two halves have genuinely
different risk. The shipped half is ~70 lines of frontend and touches no Rust
at all. The planned half needs a scroll-spy, which is the one thing a
container that cannot run a perf tier has no business landing blind — and, on
reflection, it is the weaker answer to the question the idea actually asks.
That reasoning is `docs/plans/reading-progress-indicator.md`.

## The open questions, answered

**Percentage, heading count, or both? → Percentage.**

A heading counter measures *structure*; "how far am I" is a *distance*
question. On the documents dreamd is pointed at, heading sizes are wildly
uneven — `## Architecture` in this repo's own `CLAUDE.md` is eight screens,
`## Docs` is half of one — so "section 3 of 12" is confidently wrong as a
position estimate, which is worse than an honest 41%. And the structural
answer already has a home: `#outline-panel` shipped in `790a883` and shows
every heading with its context. Percentage is the one thing that surface does
not already say.

**Titlebar, edge strip, or elsewhere? → Both, split by what each one is.**

- `#progress-pct` — a fixed-width readout in `#tb-actions`, left of the
  contents icon. This is *chrome*: a number you consult.
- `#progress-rail` — a 3px hairline pinned to the bottom edge of the reading
  pane, filling left to right. This is the *ambient* cue: the thickness-of-
  pages feel the idea file asks for, readable without being looked at.

**View mode (`308bb46`) — the deliberate call.** The rail **stays**, the
percentage **goes**. `body.view-mode` hides the app's *controls*; a 3px
progress hairline is not a control, it is the reading aid a reader turned the
chrome off to be alone with. The readout goes because it lives in the titlebar
and reading a number is an act of consulting the app rather than the document.
So view mode is left holding the calmer, more book-like half — which is the
right direction for what Ctrl+M is for. The omission is called out in a comment
on the `body.view-mode` block in `ui/index.html`, since that list is exactly
where someone would later "fix" it by adding `#progress-rail`.

**Not a scrubber.** `pointer-events: none` on the rail, and the readout is a
`<span>`, not a button. Getting around the document is the outline panel's job;
this one is read-only by construction.

## What was done

**Pure frontend. No Rust, no new `#[tauri::command]`, no IPC, no config key, no
keybind.** `src-tauri/` was not touched at all, so nothing here is missing a
compile check.

### Markup + CSS (`ui/index.html`)

- `<span id="progress-pct" data-tip="Reading position">` as the first child of
  `#tb-actions`. Fixed `width: 42px` (not `min-width`), `tabular-nums`,
  `contain: layout style` — a box that cannot resize is what stops a per-scroll
  text write reflowing the titlebar. `wireTooltips()` delegates on `[data-tip]`
  for any element, so the tooltip works with no new wiring.
- `<div id="progress-rail"><div id="progress-fill"></div></div>` inside
  `#main-wrap`, after `#content-scroll`. Outside the scroller on purpose —
  inside it, it would scroll away with the document. `z-index: 10` puts it
  *under* both side panels (20), which are opaque and full height: the rail
  belongs to the reading pane, so a panel covering its own strip of it is
  correct.
- Track is `--border` (the same hairline as every other divider), fill is
  `--accent`, so the indicator themes itself with no palette change. Nothing
  new was added to `ui/themes/*.css`.

### Behaviour (`ui/app.js`, new `reading progress` section)

Written on the assumption of **no perf feedback**, so the scroll path does no
layout:

- the `scroll` listener is `{ passive: true }` and does nothing but schedule;
- work is coalesced into one `requestAnimationFrame` — a trackpad burst that
  fires the event twenty times draws once;
- `scrollHeight`/`clientHeight` are **cached** in `progMax` and refreshed from
  a `ResizeObserver` over `#content-scroll` *and* `#content` (covering: a
  render, an image landing, a content-width or font change, a window resize).
  A `ResizeObserver` callback runs after layout, so the measurement read there
  is free. Per-frame work is therefore one `scrollTop` read;
- the fill moves by `transform: scaleX()` on a `will-change: transform` layer —
  compositor-only, never `width`;
- the readout is written only when the whole-number percent changes: at most
  100 text writes across a full-document scroll, each confined to its own box.

`measureProgress()` cancels any queued frame before drawing, so a render and a
scroll in the same tick cannot produce two draws. It is called from
`renderCurrent` (both the success and the render-error path), and from the
`file-removed` and `repo-changed` handlers that set `#content` directly — the
same four sites as `refreshOutline()`. In the success path it sits immediately
after `scrollEl.scrollTop = prevScroll`, which has already forced the layout it
reads, so it costs nothing extra there.

**Empty state.** With no file open, or a document that already fits on screen
(`progMax <= 0`), the rail fades out and the readout goes blank rather than
claiming 100%. A document you can see all of has no "how far" to report.

`ResizeObserver` is feature-detected; without it the indicator still works and
only goes stale on a window resize.

## Files touched

- `ui/index.html` — `#progress-pct` in the titlebar, `#progress-rail` /
  `#progress-fill` in `#main-wrap`, CSS for all three, and a note on the
  `body.view-mode` block explaining why the rail is not in it.
- `ui/app.js` — new `reading progress` section (`scheduleProgress`,
  `drawProgress`, `measureProgress`), wiring in `wireUi()`, four
  `measureProgress()` call sites.
- `README.md` — Usage: a **Reading position** bullet, and the **Top bar**
  bullet now names the readout.
- `docs/plans/reading-progress-indicator.md` — the heading-aware variant.

**`src-tauri/` was NOT touched**, and neither was `perf/harness/` (nothing
there references the titlebar, and no keybind or `KEY_ACTIONS` row was added,
so `every action gets a row` is unaffected).

## Verification

- `node --check ui/app.js` passes.
- The build gate was not needed — no Rust file changed. Confirmed with
  `git diff --stat`.
- Everything else is **by eye**: no Tauri/WebKitGTK run and no Chromium
  (Playwright's Chromium download is blocked by this container's proxy), so
  the CSS has never been rendered and the scroll path has never executed.

## Left open

- **The rail and the readout have not been seen.** Most likely thing to want
  tuning on first sight: the rail's 3px height and whether `--accent` at full
  strength is too loud for a "calm" cue in view mode. Both are one CSS value.
- **The `ResizeObserver` on `#content` fires on every render** (the height
  changes), which means `measureProgress` runs twice per render — once from the
  observer, once from the explicit call. Both are idempotent and the second is
  free, but if a profile ever shows it, the explicit call in `renderCurrent` is
  the removable one.
- **Rubber-band overscroll** is clamped, but WKWebView's elastic scroll was not
  observed; if the rail visibly sticks at 0/100% during a bounce, that is
  expected and correct.
- **The heading-aware "section 3 of 12" variant** is designed but not built —
  `docs/plans/reading-progress-indicator.md`, which recommends the tooltip
  route over an `IntersectionObserver` scroll-spy and says why.

perf not run - pending manual check on the author's machine
