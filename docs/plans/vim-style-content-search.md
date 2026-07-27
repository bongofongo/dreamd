# Plan: vim-style `/` content search

Companion to `ideas/done/vim-style-content-search.md`. **Nothing was built.** This is
a design for the whole feature, written because the build is small in lines but
wide in surface — it touches the rendered DOM that highlight anchoring reads,
the one pending-prefix state machine in the keydown dispatcher, three keymap
fields that cascade into Rust and two harness mirrors, and the print
stylesheet — and none of it is testable without a webview.

Read the first section before anything else. It overturns the idea file's
central decision.

---

## 1. The idea file's "Decision: raw source" is wrong, and the repo already
##    contains the reason

The idea file commits to searching the **raw markdown source** "for speed", on
the grounds that "the source string is already resident (it's what
`locate`/anchoring reads anyway)". It then spends three paragraphs
enumerating what that costs: per-match DOM mapping on every keystroke, syntax
noise from `[text](url)` and fence markers, and the `te**s**t` blind spot where
a match visible on screen is invisible in source.

Two facts kill the premise.

**Fact one: the raw source is not resident in the frontend, and never has
been.** Every `read_source` call in the app happens Rust-side —
`main.rs:168` (`render_markdown`), `:204` (`add_highlight`), `:244`
(`reanchor`). There is no `#[tauri::command]` that hands markdown source to
JavaScript; `render_markdown` returns **HTML**. So "already resident" is true of
the *backend* and false of the frontend, which is where a `/` bar lives.
Searching raw source would mean either a new command returning the whole file
into JS (a new IPC surface, plus the whole DOM-mapping problem intact) or a
per-keystroke Rust round trip. Both are strictly more work than the alternative
below, before any of the three costs the idea file already conceded.

**Fact two: the mapping machinery the idea file calls "the main hidden cost"
already exists, is already fast, and is already used.** `ui/app.js:712`:

```js
function scanTextNodes(container) {
  const nodes = [], starts = [], parts = [];
  let total = 0;
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  for (let n; (n = walker.nextNode()); ) {
    nodes.push(n); starts.push(total); parts.push(n.nodeValue);
    total += n.nodeValue.length;
  }
  return { nodes, starts, text: parts.join("") };
}
```

That is the rendered document flattened into one string, plus the index needed
to turn any offset in it back into a `(node, offset)` pair —
`nodeIndexAt(starts, at)` at `:743` is the binary search, already written.
`applyHighlights` builds it whenever there are more than four highlights to
place, and the comment above `SCAN_THRESHOLD` records the measurement:
**"flattening a 2MB document costs ~4ms whether there is one quote or five
hundred"** (Chromium harness, relative signal only).

So the honest comparison is:

| | raw source (idea file) | flattened rendered text |
|---|---|---|
| getting the haystack | new IPC command, whole file into JS | `scanTextNodes(contentEl)`, exists, ~4ms at 2MB, **once per render** |
| per keystroke | string/regex scan | string/regex scan — *identical* |
| offset → DOM position | unsolved; "the main hidden cost" | `nodeIndexAt`, exists, O(log n) |
| syntax noise | yes — `[text](url)`, fences, frontmatter, HTML comments | none; the haystack *is* what the reader sees |
| `te**s**t` blind spot | yes | none — the flat string reads `test` |
| new code | index build + a mapping layer that does not exist | ~0 for the haystack |

The rendered path is faster in the only place that matters (per keystroke it is
the same scan; the flatten amortises over a render, not a keystroke), simpler,
and does not have either of the two correctness holes. **Search the flattened
rendered text. Cache the index per render.**

This is unmeasured here — no perf tier was run — but note that the speed
argument is not what decides it. Even if raw source won a benchmark, it would be
winning on the wrong axis: the flatten is amortised per *render*, and a reader
types many keystrokes per render.

### 1a. The one thing raw source would have been better at

Nothing this feature needs. Worth stating so the trade is explicit: a raw-source
search is the only way to find text that markdown *removes* — a link's URL, an
HTML comment, YAML frontmatter. That is a different feature ("search the
source"), it is not what `/` means in a reader, and it should not be smuggled
into this one.

---

## 2. Recommendation on scope: build it, but not tonight

The feature is worth building and the design below is complete. It was **not**
implemented in this pass for four reasons, in order of weight:

1. **It paints the DOM that highlight anchoring reads.** Section 4 shows a
   concrete corruption path — a find hit wrapped across an existing `mark.hl`
   can split that mark into two elements sharing one `data-id`, which breaks
   `focusHighlight`'s `querySelector` and survives the unwrap. The design that
   avoids it (CSS Custom Highlight API, zero DOM mutation) cannot be
   feature-detected, exercised, or seen at all without a webview, and there is
   none here.
2. **No webview, no Chromium, no perf tier.** `perf/harness` needs a Playwright
   install this environment cannot do, and every perf tier was off-limits this
   run. Incremental search is a per-keystroke path over a possibly-2MB string;
   shipping it unmeasured is the exact thing the working practices forbid.
3. **`CLAUDE.md` scopes content search as v2.** Not a veto, but it means the
   author has not chosen the trade-offs below, and several of them
   (smart-case? bare `n`/`N`? regex on by default?) are taste calls that belong
   to them.
4. **Wide, not deep.** Five files across two languages plus two harness
   mirrors, and the settings panel's `rows === 19` assertion moves — an
   assertion that can only be checked in the harness that will not run.

There is no useful narrow slice. A bar that finds matches but does not paint
them is worse than no bar; the keymap fields without the feature are dead
config; the index cache without a consumer is speculation.

---

## 3. Shape

```
┌──────────────────────────────────────────┐
│ titlebar                                 │
├────────┬─────────────────────────────────┤
│ side   │ #content-scroll   (flex: 1)     │
│ bar    │                                 │
│        │ ─── #progress-rail ─────────────│
│        ├─────────────────────────────────┤
│        │ /pattern▏      [.*]  3/17  ‹ › ✕│  #find-bar
└────────┴─────────────────────────────────┘
```

`#find-bar` is a **sibling inside `#main-wrap`**, after `#progress-rail` — not
an overlay, not a repurposed `#palette-overlay`. `#main-wrap` is already
`display: flex; flex-direction: column` with `#content-scroll` at `flex: 1`
(`ui/index.html:137,141`), so a plain block child after it docks at the foot and
**shrinks the scroller** rather than covering it. That is better than vim's
overlay: the last line of the document is never hidden behind the bar you are
typing into. Closed state is `display: none`, so it costs no layout.

Two consequences worth naming:

- **It is not in the overlay guard.** `wireKeys` returns early when
  `#palette-overlay`, `#annot-overlay`, `#confirm-overlay` or `#settings-overlay`
  is open (`app.js:1708`). `#find-bar` must **not** be added to that list. It is
  a focused `<input>`, so the existing `isEditable(e.target)` guard at `:1721`
  already stops every bare-letter binding from firing while you type — including
  `/` itself, so typing a literal slash into the pattern works with no new code.
  Adding it to the overlay guard would instead kill `Escape`-to-view-mode
  semantics and gain nothing.
- **View mode must not hide it.** `body.view-mode` hides the titlebar, sidebar
  and both panels (`index.html:337-341`). `#find-bar` stays visible for the same
  reason `#progress-rail` is deliberately absent from that list: it belongs to
  the reading pane, and hiding it would strand a reader who just pressed `/`
  with no titlebar to click and no bar to type into.

---

## 4. Painting matches — the decision that carries the risk

Requirement, non-negotiable: **search must not corrupt or destabilise
highlights.**

### Rejected: wrap each match in `<mark class="find-hit">`

The obvious approach, and it is *nearly* safe. Two of the three fears are
unfounded and should be written down so nobody re-litigates them:

- **It does not corrupt captured quotes.** A `<mark>` wrap re-parents existing
  text; it adds no text node. `range.toString()`, `getSelection().toString()`
  and `selectionContext`'s node-by-node concatenation are all byte-identical
  before and after. This is *not* the `558feae` hazard — that one was about
  injected chrome contributing text; this contributes none.
- **It does not race `applyHighlights`.** Placement only ever runs on a fresh
  `innerHTML` inside `renderCurrent`, which wipes find marks along with
  everything else.

The third is real:

- **It can split an existing `mark.hl` in two.** `wrapRange` (`app.js:811`)
  falls back to `extractContents()` + `insertNode()` when `surroundContents`
  throws on a partially-selected node. A find hit that starts inside a
  `mark.hl` and ends outside it takes that path, and the result is two
  `mark.hl` elements carrying the same `data-id`. `focusHighlight`
  (`app.js:906`) does `querySelector('mark.hl[data-id="…"]')` and finds only the
  first; the click handler at `:1605` resolves half a highlight; and unwrapping
  the find hit later does **not** merge them back. That is durable corruption of
  session state the user cannot see happening.

  It is fixable — refuse to paint any match whose range crosses an element
  boundary, the same constraint `locateInNodes` already imposes on quotes. But
  that constraint throws away precisely the class of match rendered-text search
  *gains* over raw source: `te**s**t` spans two text nodes and would go unpainted.

### Recommended: CSS Custom Highlight API, zero DOM mutation

```js
const hlAll  = new Highlight(...ranges);
const hlCur  = new Highlight(ranges[current]);
CSS.highlights.set("dreamd-find", hlAll);
CSS.highlights.set("dreamd-find-current", hlCur);
```

```css
::highlight(dreamd-find)         { background: var(--find, #3a5a8c); color: inherit; }
::highlight(dreamd-find-current) { background: var(--find-cur, #f2d16b); color: var(--hl-text, #1a1a1a); }
```

`Highlight` takes `Range`s that may span element boundaries freely and paints
them without touching the DOM at all. Clearing is `CSS.highlights.delete(name)`.

This satisfies the hard requirement **by construction**: there is nothing to
corrupt, because nothing is mutated. No text nodes split, no marks re-parented,
no unwrap step to get wrong, no `normalize()` to invalidate cached node
references, and `te**s**t` paints correctly across the boundary. It is also the
API browsers shipped *for* find-on-page.

Availability: WebKit from Safari 17.2 (Dec 2023), WebKitGTK 2.44. dreamd ships
signed macOS builds today, so the overwhelming majority of the audience has it.

**Degradation, when `window.CSS?.highlights` is undefined:** the bar still
opens, still counts, and `n`/`N` still scroll to each match — the search works,
it just is not painted, and the count in the bar becomes the only feedback.
Show a one-time toast saying so. Do **not** fall back to DOM wrapping: that
trades a structural guarantee about the app's core loop for a visual on old
WebKit, and it is the wrong way round. If the no-paint fallback turns out to
matter in practice, the smallest safe upgrade is to wrap **only the current
match**, single-text-node only, unwrapping on every move — one mutation at a
time instead of N — which is a follow-up, not v1.

**Print.** Add to the hide list at `index.html:594-597`:

```css
#find-bar { display: none !important; }
```

and neutralise the paint next to the existing `#content mark.hl` block, for the
same reason that block exists (session state must not be baked into a durable
PDF):

```css
::highlight(dreamd-find),
::highlight(dreamd-find-current) { background: transparent; color: inherit; }
```

`#print-css` must stay the last `<style>` in `<head>` (commit `3218fdf`);
nothing here changes that.

---

## 5. The search itself

State, all in memory, all dying with the process (tenet 2 — nothing here is
written to `~/.config/dreamd/`):

```js
let findIndex   = null;   // { nodes, starts, text } | null   — per render
let findQuery   = "";
let findRegex   = false;  // the .* toggle
let findRanges  = [];     // Range[]
let findCurrent = -1;
```

**Haystack.** `findIndex ||= scanTextNodes(contentEl)`. Built lazily on the
first search after a render, so a session that never presses `/` pays nothing.
Note `scanTextNodes` walks every text node under `#content`, including inside
`<pre>` (searching code is right) and including any `mark.hl` (the text reads
through the wrapper). It sees **zero** text from the injected copy buttons —
`558feae`'s "zero text nodes" invariant, written for selection, now protects
search too. It does not reach `#stale-rail`, which is a sibling of `#content`.

**Matching — one code path for literal and regex.**

```js
function findCompile(q, regex) {
  const src = regex ? q : q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  // Smart case: an all-lowercase pattern is case-insensitive; any uppercase
  // makes it exact. Vim's `smartcase`, and the right default for a reader.
  const flags = "g" + (/[A-Z]/.test(q) ? "" : "i");
  return new RegExp(src, flags);   // caller wraps in try/catch
}
```

Escaping the literal into a `RegExp` rather than using `indexOf` +
`toLowerCase()` is deliberate: case-folding both haystack and needle can change
string *length* for some Unicode (`İ`), which silently corrupts every offset
after it. The regex engine indexes the original string, so offsets are always
sound.

**Scanning**, with the two guards a per-keystroke regex needs:

```js
const MAX_HITS = 2000;   // bounds `.` over a 2MB document
function findScan(re, text) {
  const out = [];
  re.lastIndex = 0;
  for (let m; (m = re.exec(text)); ) {
    if (m[0].length === 0) { re.lastIndex++; continue; }  // `a*` would spin forever
    out.push([m.index, m.index + m[0].length]);
    if (out.length >= MAX_HITS) break;
  }
  return out;
}
```

**Offset span → `Range`**, reusing the existing binary search verbatim:

```js
function findRange(idx, start, end) {
  const i = nodeIndexAt(idx.starts, start);
  const j = nodeIndexAt(idx.starts, end - 1);
  const r = document.createRange();
  r.setStart(idx.nodes[i], start - idx.starts[i]);
  r.setEnd(idx.nodes[j], end - idx.starts[j]);
  return r;
}
```

Unlike `locateInNodes`, this does **not** skip spans that straddle a node
boundary — that is the whole point, and the Custom Highlight API is what makes
it safe.

**Invalid regex.** `findCompile` throws mid-keystroke on `(`, `[`, `*` and
every other half-typed pattern. Catch it, put `.invalid` on the bar (red left
border, the same `--stale` red), write `bad pattern` into `#find-count`, clear
`findRanges` and the paint. No toast — the reader is mid-keystroke and a toast
per character is noise.

**Debounce.** ~60ms on `input`. Below the threshold where typing feels laggy,
and it collapses a burst of keystrokes into one scan of what may be a megabyte.
Unmeasured — this number is a guess and is the first thing to check on a real
machine with `perf/harness`'s large corpus.

**Incremental search.** As the pattern changes, jump to the first match at or
after the current scroll position (vim's `incsearch`), not to the first match in
the document. Falls back to the first match if there is none below.

**Scrolling to a match.** `#content-scroll` is the scroller, never the window:

```js
const r = findRanges[findCurrent].getBoundingClientRect();
const host = scrollEl.getBoundingClientRect();
scrollEl.scrollTop += r.top - host.top - scrollEl.clientHeight / 3;
```

Instant, no `behavior: "smooth"` — matching the policy stated on
`jumpTop`/`jumpBottom` (`app.js:1138`), and because `n` held down should step,
not animate. A range with a zero-height rect (a match inside a
`display: none` subtree, if one ever exists) should be skipped rather than
scrolled to.

**Re-render invalidation — do not skip this.** A `Range` into a detached node is
a live object that silently scrolls nowhere. `renderCurrent` replaces
`#content`'s `innerHTML`, so every stored range dies on `file-changed`, on a
theme switch, and on every `openFile`. Therefore:

- Set `findIndex = null; findRanges = []; findCurrent = -1;` and
  `CSS.highlights.delete(...)` **at the top of `renderCurrent`**, before the
  `innerHTML` write.
- After `applyHighlights` and before `measureProgress`, if `findQuery` is
  non-empty **and the file is the same one** (a watcher re-render, not a
  navigation), re-run the scan against the fresh DOM and clamp `findCurrent`.
  This is the search equivalent of `reanchor`, and without it a save under an
  open find bar leaves stale paint and dead `n`.
- On `openFile` (a different document), clear `findQuery` too. Vim keeps the
  last pattern across buffers; here the pattern survives in the input box but
  the match set is rebuilt from scratch anyway, so either behaviour is
  defensible — recommend keeping the pattern and re-running it, which is what
  vim does and costs nothing extra given the point above.

---

## 6. Keys, and the dispatcher

Three new bindings. Defaults `find = "/"`, `find_next = "n"`,
`find_prev = "Shift+N"`.

`/` joins `]` and `[` as bare punctuation; `n`/`N` join `h`, `m` and `'` as bare
letters. Every one of them dispatches **below** the `isEditable` guard, so none
can reach a reader typing into the annotation textarea, the palette input, or
the find input itself.

**Interaction with the marks state machine (`583a467`) — checked, no conflict
in either direction.**

- **`/` cannot be swallowed.** With `m` armed, `consumeMarkKey` runs first
  (`:1727`), sees `/` fail `/^[0-9a-z]$/i`, cancels the chord and returns
  `false` **unprevented**. The `find` binding below then fires. Correct
  outcome — the abandoned mark costs the mark, not the keystroke — and it means
  the `find` dispatch needs **no** explicit `clearMark()`, unlike `palette` and
  `settings` which sit *above* `consumeMarkKey` and therefore must clear by
  hand.
- **`n` and `N` are correctly swallowed.** With `m` armed, `m` then `n` sets
  mark `n`; it does not step the search. That is vim's precedence and the right
  one. Nothing to change.
- **The find bar swallows nothing.** It adds no cross-event state; its three
  bindings are ordinary single combos matched by the existing `matchCombo`,
  which requires an exact modifier match (`"Shift+N"` matches `e.key === "N"`
  with only Shift held; bare `n` cannot match it, and `Ctrl+N` — `palette_next` —
  cannot match either).

**Dispatch order**, unchanged except for three lines added to the existing
single-combo chain, next to the file-stepping pair:

```
Escape → overlay guard → palette/settings → isEditable → consumeMarkKey →
  … existing chain …
  find / find_next / find_prev
```

**Escape.** The find bar claims it *after* `cancelRecording()` and `pendingMark`
and *before* view mode. In `wireKeys`'s Escape branch: add `"find-bar"` to the
`claimed` array (`:1693`) and a `closeFind()` next to `closePalette()`. Being in
`claimed` is what makes Escape close the bar and leave view mode alone, which is
the same rule every other panel follows.

**Inside the input:**

- `Enter` — commit: jump to the current match, close the bar, **keep the match
  set and the paint live** so `n`/`N` continue to work. This is vim.
- `Shift+Enter` — previous match, bar stays open.
- `Escape` — cancel: close, clear the paint, clear the match set. Also vim.
- `Ctrl+Enter` is `send_stack` and must not be shadowed; the input's own
  handler should only claim bare and Shift `Enter`.

**Keymap plumbing — the five places a field name has to exist.** All five, or
the settings panel and the harness disagree:

1. `src-tauri/src/config.rs` — three `pub` fields on `Keymap` (~`:131`) and
   three lines in `Keymap::default()` (~`:187`), with the same
   comment-the-judgement-call style as `set_mark`/`jump_mark`.
2. `ui/app.js` `KEY_ACTIONS` (`:1872`) — three rows.
3. `ui/app.js` `wireKeys` — three `matchCombo` lines.
4. `perf/harness/ui-check.mjs` — the **two** `KEYMAP` literals (`:51`, `:362`),
   and `rows === 19` → `rows === 22` at `:181`. The row count is
   `KEY_ACTIONS.length + 1`; the `+1` is the `quick_highlight` checkbox row
   `renderKeys` appends after the loop.
5. `perf/harness/lib/fixtures.mjs` — the `KEYMAP` mirror (~`:124`).

Plus `README.md`'s keybind table and config sample, which every previous keybind
change has updated.

No Rust *logic* changes at all — three strings and their defaults. Worth saying
plainly: **this feature does not touch `src/search.rs`, `nucleo`, or the
`SearchIndex`, and does not open the v2 content-index scope.** It searches the
one document that is open, in the frontend. Cross-file content search remains
unbuilt and unplanned here.

---

## 7. Markup and CSS

```html
<!-- inside #main-wrap, after #progress-rail -->
<div id="find-bar">
  <span class="find-sigil">/</span>
  <input id="find-input" autocomplete="off" spellcheck="false"
         aria-label="Find in document" placeholder="Find in document…" />
  <button id="find-regex" class="icon" data-tip="Regular expression"
          aria-pressed="false">.*</button>
  <span id="find-count" aria-live="polite"></span>
  <button id="find-prev" class="icon" data-tip="Previous match" data-tip-key="find_prev">‹</button>
  <button id="find-next" class="icon" data-tip="Next match" data-tip-key="find_next">›</button>
  <button id="find-close" class="icon" data-tip="Close find">✕</button>
</div>
```

`#find-bar { display: none }` / `#find-bar.open { display: flex }`, a
`border-top: 1px solid var(--border)`, and the same `--btn-bg` / `--muted`
vocabulary as the rest of the chrome so every bundled and user palette gets it
free (tenet 5). Two new palette variables, `--find` and `--find-cur`, each with
an inline fallback in the `::highlight` rules exactly as `mark.hl` does with
`var(--hl, #f2d16b)` — so a pre-existing user palette that has never heard of
them still renders.

Do not put the bar inside `#content-scroll`: it would scroll away with the text.

---

## 8. Verification, for whoever builds it

Nothing below could be run in the environment this plan was written in.

- `cargo build --lib` clean; the bin's error count must stay at exactly 5 (the
  pre-existing macOS gating in `main.rs:863-865`).
- `node --check ui/app.js`.
- The scan/compile/range functions are pure and can be driven from Node against
  a fake `{nodes, starts, text}` — grep them out of `ui/app.js` itself rather
  than copying, the way the marks harness did. Cases worth asserting: smart
  case both ways; a literal containing regex metacharacters; an invalid regex;
  `a*` and `(?:)` (zero-length) terminating; `MAX_HITS`; a match spanning two
  text nodes producing a range with different start and end containers.
- `perf/harness/ui-check.mjs` needs a case that opens the bar, types, and checks
  `#find-count` — and the `rows` bump.
- **In a real webview, by hand:** that `CSS.highlights` paints in WKWebView at
  all; that `/` reaches the handler on the author's layout; that Escape order is
  right with view mode on; that a `file-changed` save under an open bar
  re-scans instead of stranding dead ranges; and that placing a highlight over
  a painted match still anchors — the one thing this whole design is built to
  protect.
- **Perf, before it lands:** `/perf-quick` after the edit and `/perf-pass`
  before the commit. The specific number to watch is `render_total`, which now
  carries a conditional re-scan, and the debounce constant in §5.

## 9. Left open

- **Regex off by default, in memory only.** The `.*` toggle does not persist.
  Tenet 2 permits a preference and this is arguably one; deliberately not taken,
  because a persisted regex mode means a pattern typed in a later session is
  silently interpreted, which is the surprising direction.
- **No `?` for backward search.** Vim has it; `n`/`N` cover the need and `?` is
  one more bare-punctuation claim on a keyspace that now has `/`, `]`, `[` and
  `'` in it.
- **No match density on the progress rail.** Firefox-style tick marks along
  `#progress-rail` would be genuinely good here, and the rail already exists —
  but it is `pointer-events: none` by explicit design ("a bookmark, not a
  scrubber", `index.html:205`), and adding ticks starts an argument about
  whether they are clickable. Separate decision.
- **Search does not reach collapsed or unopened files.** By design; see §6.
- **`MAX_HITS = 2000` is a guess**, as is the 60ms debounce.
