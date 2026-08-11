# ui-reading — 2026-08-11

Propose-only sweep of `ui/app.js`'s render, highlight, placement, anchoring and
scroll paths. **No code changed.** Everything below is a proposal; whether any of
it lands is the user's call.

Read: `renderCurrent`/`interceptLinks`/`decorateCodeBlocks`/`scrollToFragment`
(745–981), the whole highlight block (986–1330), overlap + resize (1333–1588),
and the scroll/glide/frame/jump-history block (1994–2317).

## Pass 1 — verify

Every claim CLAUDE.md makes about this area was checked against the code. Almost
all of them hold, which is the main result:

| Claim (CLAUDE.md) | Verdict |
|---|---|
| `wrapByWalk` and `locateInNodes` both look inside a *single* text node | true — 1161, 1196 |
| `placeAcrossNodes` is the fallback **both** placers hand their misses to | true — 1057 and 1062 both push to `crossNode`, drained at 1077 |
| one `<mark>` per text-node slice, sharing the id | true — 1115–1134 |
| `data-run` (`start`/`mid`/`end`) squares the interior edges | true — 1121, and `mark.hl[data-run]` at index.html:373–375 |
| a placement failure **claims nothing** | true — 1052 chips only on `state === "stale"`; 1113 `continue`s silently |
| the frontend sends `getSelection().toString()` | true — 1374, 1577 |
| Escape ranks below every overlay and above view mode | true — 4796–4812, and the `claimed` guard is what orders it |
| `clearHighlights` ends resize mode | true — 1007 |
| the commit re-checks overlap excluding the mark itself | true — 1573 |
| the stack panel's `⤢` opens the file, scrolls the mark in, arms | true — 1748–1758 → `resizeFromStack` 1534 |
| the pop handler and `saveAnnot` call `repaintHighlights` beside `refreshStack` | true — 1742, 1462 |
| resize is asserted in `ui-check.mjs` | true — `#resize-hint` at ui-check.mjs:3225 |
| pre-paint `data-mode` bootstrap at the top of `app.js` (CSP) | true — 17–19 |

Two pieces of drift, both small:

**D1 — the line count.** CLAUDE.md:788 calls `ui/app.js` "5,700 lines". It is
**5,975**. Proposed: `~6,000 lines`, which stops the number needing a sweep every
time the file moves.

**D2 — "`clearHighlights` and `deleteHighlight` both use `querySelectorAll`"**
(CLAUDE.md:660). Still true, but it names two consumers where there are now five
and misses the thing that makes the rule hold. `marksFor` (1363) is the shared
helper, and `deleteHighlight` (1483), `armResize` (1504), `endResize` (1547) and
`resizeFromStack` (1536) all go through it; only `clearHighlights` (1008) queries
directly, because it wants every mark rather than one id's. Proposed replacement
for that last sentence:

> Anything consuming a mark must therefore tolerate several per id — every
> by-id consumer goes through `marksFor`, and `clearHighlights` sweeps
> `querySelectorAll("mark.hl")` outright.

## Pass 2 — simplify

### F1. `wrapRange`'s `stale` parameter is dead — and so is the CSS behind it

All four call sites pass `false`: 1072, 1132, 1170, 1400. They cannot pass
anything else, because `applyHighlights` sends a stale mark to the rail and
`continue`s (1052) before any placement runs — a stale highlight is never painted
in the document at all. `ui-check.mjs` agrees: it counts `.stale-chip`s and never
looks for a `mark.hl.stale`.

So `mark.className = "hl" + (stale ? " stale" : "")` (1307) is a branch with one
reachable arm, and `mark.hl.stale` at index.html:361 is a rule nothing can match.

Sketch:

```diff
-function wrapRange(range, id, stale, prior) {
+function wrapRange(range, id, prior) {
   const mark = document.createElement("mark");
-  mark.className = "hl" + (stale ? " stale" : "");
+  mark.className = "hl";
```

and the four call sites drop their `false`. −5 lines.

Two notes, both deliberately left as notes rather than done:

- `index.html` is area `ui-panels`, not this one. Its `mark.hl.stale` rule (361)
  and the `#content mark.hl.stale` half of the print rule (1567) are the
  unreachable other end of this and want the same commit — flagging rather than
  reaching across.
- The reason for the *rail* is intact and documented at 1219–1227. This removes
  the vestige of the older design where stale marks painted red inline, not the
  design that replaced it.

### F2. `applyHighlights` flattens the document twice, and carries two placers for one job

`segmentsIn`'s own doc comment (1137–1141) says it plainly: "a quote inside a
single node yields exactly the one slice `locateInNodes` would have returned —
this is a generalisation of that, not a second code path beside it." But both
paths are in the file, and above `SCAN_THRESHOLD` a render with any cross-node
quote flattens `contentEl` twice — once at 1041, once at 1077 — because the
cross-node placements are computed *after* the first pass has already wrapped and
split nodes.

Nothing forces that ordering. In the above-threshold path no wrapping happens
until 1068, after every `locateInNodes` call has returned; if the cross-node
quotes were sliced against the *same* flatten and all segments sorted back to
front together, one pass would place everything and the second `scanTextNodes`
would go away with `locateInNodes`.

Sketch (above-threshold arm only — the `!doc` arm keeps `wrapByWalk`, which wins
below the threshold precisely by stopping at the first hit):

```js
// one flatten, one sort, one wrap pass
const segments = [];
for (const h of list) {
  if (h.state === "stale") { addStaleChip(h); continue; }
  const quote = h.quote.trim();
  if (!doc) { if (!wrapByWalk(contentEl, quote, h.id, h.prior)) crossNode.push(h); continue; }
  const at = doc.text.indexOf(quote);
  if (at < 0) continue;                       // not on screen; the store keeps it
  pushSlices(segments, doc, at, quote);       // = placeAcrossNodes's inner loop
}
segments.sort((a, b) => b.at - a.at);
for (const s of segments) { /* wrapRange + data-run, as today */ }
if (crossNode.length) placeAcrossNodes(scanTextNodes(contentEl), crossNode);
```

`placeAcrossNodes` stays, for the below-threshold remainder. `locateInNodes`
(1193–1206) goes; so does the second flatten on the hot path. Roughly −15 lines.

**Behaviour delta, and it is the reason this is a proposal and not a diff.** For
a quote that occurs twice, where the *first* occurrence straddles a node boundary
and a later one sits wholly inside one node, today's `locateInNodes` skips to the
later copy and the unified version paints the first. The first is the better
answer — it is the one `markdown::locate` anchors to, so the paint and the stored
line numbers would agree where today they can disagree — but it is a change to
what the reader sees, in a file the harnesses cannot prove the paint of.

Needs checking by eye: a document with `**bold** mid-sentence` repeated, one mark
on each copy; a mark spanning a link; three-slice runs still reading as one phrase
(`data-run`, ui-check.mjs:1249–1279 covers the shape but not the pixels).

**Measured path.** `apply_highlights` is a `perf.span` (801) and part of
`save_to_paint`. If F2 lands, `/perf-quick` on the author's machine before merge.

### F3. Three copies of "the open document went away", and they have drifted apart

- `doDeleteFile` (3206–3210): `currentFile = null`, empty-state `innerHTML`, clear the rail.
- `file-removed` (4548–4554): the same three, plus `refreshOutline()` and `resetFind()`.
- `repo-changed` (4583–4588): the same three, plus `refreshOutline()`, with `resetFind()` hoisted above at 4577.

The delete path is the odd one, and not harmlessly. It nulls `currentFile`
*before* the watcher's `file-removed` arrives, so that listener's
`e.payload.path === currentFile` test is `path === null` and its branch — the one
carrying `refreshOutline` and `resetFind` — never runs. (`forgetPath` is fine: it
sits outside the branch at 4547, deliberately, with a comment saying why.)

Reader-visible, when the file being deleted is the one on screen:

- outline card open → it keeps listing the deleted document's headings until the
  next file is opened;
- find bar open → the count and `n` survive over an empty pane, pointing into a
  DOM that has been replaced.

Proposed: one helper next to `renderCurrent`, used by all three.

```js
/// The open document is gone — deleted, removed under the watcher, or left
/// behind by a repo swap. Everything keyed to it goes at once; splitting this
/// across three call sites is how the delete path came to keep a stale outline.
function clearOpenDocument(message) {
  currentFile = null;
  contentEl.innerHTML = `<div class="empty">${escapeHtml(message)}</div>`;
  staleRail.innerHTML = "";
  refreshOutline();
  resetFind();
}
```

Net ≈ −6 lines and one class of divergence. `repo-changed` already calls
`resetFind()` earlier for its own reason (a pattern must not survive a repo
swap) — a second call is a no-op, but worth confirming that reading of
`resetFind` before landing.

Needs checking by eye: delete the open file with the outline card open, and again
with the find bar open on a matching pattern.

### F4 (minor). The rail is cleared twice on every repaint

`clearHighlights` clears `staleRail` (1010) and `applyHighlights` clears it again
(1033). Every caller of the first immediately calls the second, and the second
needs its own clear because `renderCurrent` reaches it without the first (a fresh
`innerHTML` wipes the marks but not the rail, which lives outside `#content`).
The line in `clearHighlights` is the removable one. One line; only worth taking
alongside F3, which touches the same rail.

### F5 (minor). `addStaleChip` builds user text with `innerHTML`

1231–1232 uses `innerHTML` with `escapeHtml` around the quote. Correct as it
stands — tenet 4 is not at risk — but the stack panel's own note (1787) states
the convention for this exact kind of text: "`textContent` throughout, so none of
this needs `escapeHtml` at all". Two elements and one `append` would put the chip
on the same footing. Cosmetic; listed for completeness.

## Read and found clean

The scroll block — `jumpTo`/`glideBy`/`stepGlide`/`endGlide`, `lineHeight`,
`here`/`restoreFrame`, the mark, and the jump history — has nothing to propose.
The hijack check (2116–2125), the per-frame re-clamp (2130), the `dt` clamp
(2140) and the `expected`-starts-at-current-scroll subtlety (2095–2098) each
carry the reason they exist, `maxScroll`'s single caller is a name worth keeping,
and no function in it is dead.

## Adjacent, out of area — one for `ui-panels`

`ui/index.html`:1558 says highlights "are session state that dies with the
process (tenet 2)" as the justification for printing them as plain text. The
first half is false since step 4: marks persist in `marks/<basename>-<16hex>.json`
and CLAUDE.md's tenet 2 says so. The *conclusion* is still right — baking a
session's markup into a durable PDF is a category error either way, and the
reproducibility argument in the same comment carries it alone — so this is a
wrong reason under a correct rule, which is the kind of line that misleads a
future session. Belongs to whoever sweeps `ui-panels`; noted here rather than
fixed, per "stay in the area".
