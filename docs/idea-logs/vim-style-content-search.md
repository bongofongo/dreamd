# Vim-style `/` content search

**Planned, not implemented.** `docs/plans/vim-style-content-search.md` is the
deliverable; no code changed.

## Call: plan

Against the risk criteria this is the "touches a lot / higher risk" side, on
four counts:

1. It paints the DOM that highlight anchoring reads. The plan documents a
   concrete corruption path — a `<mark>`-wrapped find hit that partially covers
   an existing `mark.hl` takes `wrapRange`'s `extractContents` fallback and
   leaves **two** `mark.hl` elements sharing one `data-id`, which breaks
   `focusHighlight`'s `querySelector` and survives the unwrap. Durable, silent
   damage to session state.
2. Five files across two languages (`ui/index.html`, `ui/app.js`,
   `src-tauri/src/config.rs`, `perf/harness/ui-check.mjs`,
   `perf/harness/lib/fixtures.mjs`, plus `README.md`), and the settings panel's
   `rows === 19` assertion moves to 22 — an assertion only checkable in a
   harness that cannot run here.
3. No webview and no Chromium, so incremental search — a per-keystroke scan over
   up to 2MB — could not be seen working or timed, and no perf tier was
   permitted this run.
4. `CLAUDE.md` scopes content search as v2, so the taste calls (smart-case?
   bare `n`/`N`? regex default?) are the author's to make.

There is no useful narrow slice: a bar that finds but does not paint is worse
than no bar, keymap fields without the feature are dead config, and the index
cache without a consumer is speculation. So: a complete design, nothing
half-built.

## The finding that matters: the idea file's central decision is wrong

`ideas/vim-style-content-search.md` commits to searching **raw markdown source**
"for speed", on the grounds that the source is "already resident", then concedes
three costs — per-match DOM mapping every keystroke, syntax noise, and the
`te**s**t` blind spot. Two facts in the repo overturn the premise:

- **The frontend has never had the raw source.** Every `read_source` is
  Rust-side (`main.rs:168`, `:204`, `:244`); `render_markdown` returns HTML.
  There is no command that hands markdown to JS. Raw-source search would need a
  *new* IPC surface before paying any of the three conceded costs.
- **The mapping the idea file calls "the main hidden cost" already exists.**
  `scanTextNodes` (`app.js:712`) flattens the rendered document to one string
  plus a node/offset index; `nodeIndexAt` (`:743`) is the binary search back.
  `applyHighlights` already uses both, and the `SCAN_THRESHOLD` comment records
  ~4ms to flatten 2MB (Chromium harness — relative signal only).

Per keystroke the two approaches run the *same* scan; the flatten amortises per
render, not per keystroke. The rendered path is therefore simpler, no slower
where it counts, and has neither the syntax-noise nor the `te**s**t` hole.
**Recommendation: search the flattened rendered text, cache the index per
render.**

## Second recommendation: paint with the CSS Custom Highlight API

`CSS.highlights.set(...)` with `::highlight()` styling paints ranges that span
element boundaries **without mutating the DOM at all** — which satisfies "search
must not corrupt or destabilise highlights" by construction rather than by
testing, and is the only way to paint a `te**s**t` match safely. When
unsupported (pre-Safari 17.2 / WebKitGTK 2.44), degrade to navigate-and-count
with no paint; explicitly do *not* fall back to DOM wrapping.

Also settled in the plan: `#find-bar` docks inside `#main-wrap` after
`#progress-rail` so it shrinks the scroller instead of covering the last line;
it stays **out** of the overlay guard (the existing `isEditable` check already
does the work, and is what makes typing a literal `/` free); it stays **visible**
in view mode, for the same reason `#progress-rail` is; and it hides in print.

## Dispatcher / marks interaction — checked, clean both ways

`583a467`'s `consumeMarkKey` runs first. With `m` armed, `/` fails
`/^[0-9a-z]$/i`, cancels the chord and falls through **unprevented**, so the
find binding still fires — and so the `find` dispatch needs no explicit
`clearMark()`, unlike `palette`/`settings` which sit above it. With `m` armed,
`n` correctly sets mark `n` rather than stepping the search, which is vim's
precedence. The find bar adds no cross-event state of its own, so it swallows
nothing. Escape claims: recording → mark → **find bar** → other overlays → view
mode, with `"find-bar"` added to the `claimed` array so view mode survives.

## Files touched

- `docs/plans/vim-style-content-search.md` — new, the whole design.
- `docs/idea-logs/vim-style-content-search.md` — this file.

No source files changed, so the build gate was not required and was not run.

## Left open

- Everything in the plan's §9: regex toggle persistence, `?` for backward
  search, match ticks on `#progress-rail`, `MAX_HITS = 2000` and the 60ms
  debounce (both guesses).
- Nothing in the plan has been seen in a webview. That `CSS.highlights` paints
  in WKWebView at all is the single assumption the design rests on and the first
  thing to check.
- Cross-file content search stays out of scope — this feature touches neither
  `src/search.rs` nor `nucleo`, and does not open `CLAUDE.md`'s v2 scope.

perf not run - pending manual check on the author's machine
