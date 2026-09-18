# ui-reading — 2026-09-18

Propose-only sweep of `ui/app.js`'s render, write, highlight, placement,
anchoring and scroll paths. **No code changed.** Everything below is a
proposal; whether any of it lands is the user's call.

Read: `writeContent`/`writeBackendBlocks`/`writeLegacyHtml`/`stagedFullWrite`
(835–1158), `renderCurrent` (1188–1353), the image/zoom passes (1429–1556),
`decorateCodeBlocks`/`scrollToFragment` (1906–1988), the whole highlight block
(1993–2600), resize (2765–2900), the scroll/glide/frame block (3286–3600), and
the three document-gone paths (4492, 5896, 5925).

The area has grown ~1,400 lines since the last sweep (2026-08-11), almost all
of it the backend-block write path and the delta framing. That is where both
the defect and the drift are.

## Pass 1 — verify

Every CLAUDE.md claim about this area was checked. The three newest paragraphs
— "A repaint keeps the marks whose blocks survived", "The marks on screen are a
set, not a query" and "A passage no block holds is looked for once" — hold in
every part, including each assertion they attribute to `ui-check.mjs`:

| Claim (CLAUDE.md) | Verdict |
|---|---|
| `wrapRange` is the only thing creating a `mark.hl`, `unwrap` the only thing taking one down | true — 2573/2588 are the only sites |
| `drawnMarks` may hold stale entries; `isConnected` is how they say so | true — `standingMarks` 2547 prunes on `isConnected` *and* `contentEl.contains` |
| `standingMarks` sorts each id's marks into document order, and ui-check asserts it | true — 2567 sort; ui-check 3917–3934 |
| reuse is verified — the marks sharing an id must still spell the quote | true — `drawnText` 2073, checked at 2244 |
| the four things ui-check asserts about a patch repaint | all four present — ui-check 3835, 3843, 3850, 3860–3864 |
| `unplaceable`'s three halves are asserted | all three — ui-check 3896, 3902, 3913 |
| `wrapByWalk` and `locateInNodes` look inside a *single* text node; `placeAcrossNodes` is the fallback both hand misses to | true — 2362, 2423; 2190/2200 both push to `crossNode` |
| `data-run` start/mid/end squares the interior edges | true — 2319, and ui-check 1394–1421 |
| a placement failure claims nothing; only `reanchor_file` says stale | true — 2235 records a memo, never a chip |
| the copy button carries no listener of its own; built once and cloned | true — 1906 proto + clone, delegation at 6035/6023 |
| zoom is `--zoom` on `<html>` plus two inline `calc()`s and the image rule; `measureImage` records `--img-w` | true — 1531, 1449; re-read on `repo-changed` at 5941 |
| the frontend sends `getSelection().toString()` | true — 2644, 2847 |
| the scroll block (glide, hijack check, `dt` clamp, frames) | unchanged and still clean |

Five pieces of drift. Two are repeats from 2026-08-11 that the propose-only
rule left unlanded.

**D1 — the save path no longer works the way the paragraph describing it says.**
CLAUDE.md:829 has `writeContent` "parses the incoming HTML into a `<template>`,
compares it block-by-block against `lastBlocks`", and :854 says "`writeContent`'s
own block diff is unchanged and still runs". In the real app neither is the
mechanism any more: `writeBackendBlocks` (863) compares *backend block strings*
that were never near the DOM, and parses only the changed span, after the diff —
its own comment puts the saving at "130ms of a 209ms save loop". The template
parse survives only on `writeLegacyHtml` (953), which is the harness stubs and
`showContentMessage`'s sibling. Proposed: say the comparison is string-to-string
over the backend's blocks and that only the changed span is parsed — and name
`lastBlocksBackend`, the flag keeping the two comparison spaces from ever being
compared against each other, which CLAUDE.md does not mention at all.

**D2 — "the record is taken inline, off the live DOM … one serialization"**
(CLAUDE.md:925). Only on the legacy path. `recordBackend` (943) stores the block
array the backend just sent; nothing is serialized. The *reason* in that
paragraph — deferring the record to after the frame cost `ipc_tree` 52ms → 2431ms
— is about ordering, still true, and must survive any rewrite of the sentence.

**D3 — `clearHighlights` has one caller, and a file open is not it**
(CLAUDE.md:891). It is called from `repaintHighlights` (2067) and nowhere else;
a file open takes the render path, whose blanket call was removed when the patch
contract landed (the comment at 1335 records that). A file open needs no clear
because the full `innerHTML` write disconnects every mark and `standingMarks`
prunes them. Proposed: drop "a file open" from the parenthetical, and say why it
needs no clear.

**D4 — "`clearHighlights` and `deleteHighlight` both use `querySelectorAll`"**
(CLAUDE.md:1000). Raised 2026-08-11, still open, and the shape has settled since:
`marksFor` (2633) is the shared by-id helper and `deleteHighlight` (2753),
`armResize` (2774), `resizeFromStack` (2806) and `endResize` (2817) all go
through it; only `clearHighlights` (2045) and `overlappingIds` (2616) query
directly, because they want every mark rather than one id's. `placeAcrossNodes`'s
own docstring (2285) carries the same stale sentence.

**D5 — `ui/app.js` is 7,403 lines**, not the 5,700 CLAUDE.md:1175 and
`.claude/skills/upkeep/SKILL.md` both give. Raised 2026-08-11 at 5,975.
Proposed then and still: `~7,400 lines`, or drop the number.

**D6 (omission, not a false claim) — staging has no architecture paragraph.**
`stagedFullWrite` (1060), `STAGE_HEAD`/`STAGE_MIN`, `writeGen` and the
rAF-plus-80ms insurance are ~100 lines carrying the first-paint story, and
CLAUDE.md names "staged write" only in passing, inside `smoke.sh`'s paragraph
(:92), where it is load-bearing for what that check proves. Four sentences in
the render section would close it.

## Pass 2 — simplify

### F0. A byte-identical save silently drops the generation, and the *next* save crosses whole

**This is a defect, not a cleanup, and it is the one thing here worth landing on
its own.** `writeBackendBlocks`' no-op arm (889–896) returns `[]` without calling
`recordBackend`, so `lastGen` keeps naming the *previous* render while the
backend has already incremented `render_gen` and replaced its slot
(`main.rs:625`, `:636`). The next request therefore echoes a generation the
backend no longer holds, `prev.gen == seen` fails, and the whole document crosses
— 4.4MB on the corpus doc, for a save that would otherwise have been a few
kilobytes. It is self-healing (the full answer re-records), so the cost is one
full payload after every no-op render, forever.

A no-op render is not exotic: the code's own comment beside the arm lists them —
a `:w` with no edit, a `touch`, a watcher double-fire — and a theme or appearance
switch re-renders identical HTML whenever the syntax theme did not change.

Sketch:

```diff
   if (
     blocks.length === lastBlocks.length &&
     blocks.every((b, i) => b === lastBlocks[i])
   ) {
+    // The blocks are unchanged but the *generation* is not: the backend
+    // bumped `render_gen` and replaced its slot for this render, so keeping
+    // the old number here makes the next request un-diffable and sends the
+    // whole document back.
+    recordBackend(blocks, gen);
     return [];
   }
```

`recordBackend` re-assigns the identical block array and the same file, so
nothing but `lastGen` actually moves.

**Verified, both directions.** A copy of `ui-check.mjs` in the session scratchpad
— the repo's own harness, launch path and one probe added, nothing in the tree
touched — was given a probe that saves a real edit immediately after the
"a save that changed nothing replaces nothing" case and counts deltas:

```
FAIL PROBE the save after a no-op save still crosses as a difference  deltas 4 -> 4
437 passed, 1 failed
```

Against a scratchpad copy of `ui/` carrying the three-line diff above, the same
run is `438 passed, 0 failed`. The existing harness cannot see this: its
`window.__deltas` assertion is taken *before* the no-op save and never again.

If this lands, the probe should land with it as a permanent assertion in
`ui-check.mjs` (one save after the no-op, delta count up by one) — that file is
not `ui/app.js` and is not propose-only.

**Measured path.** The render payload is `d:ipc_render_markdown` and
`save_to_paint`; `/perf-quick` on the author's machine before merge, though the
change can only shrink what crosses.

### F1. The widen comment is written twice, and the first copy is in the wrong place

`applyHighlights` carries the same six-line comment — "A narrowed scope widens
here, and only here…" — at 2222, above the `if (crossNode.length)` block, and
again at 2238, inside the `else if (missed.length)` branch. The widening happens
in the second one only; the first sits above the *narrow* pass, which is what it
is denying. Proposed: keep 2238, and replace 2222 with a line about what that
block actually is (the scope the walk already covered, searched first). −6 lines,
comments only.

### F2. `wrapRange`'s `stale` parameter is still dead

Unchanged from 2026-08-11: all four call sites — 2212, 2332, 2372, 2670 — pass
`false`, and nothing can pass anything else — `applyHighlights`
sends a stale mark to the rail and `continue`s (2136–2137) before any placement runs.
So `mark.className = "hl" + (stale ? " stale" : "")` (2575) has one reachable arm.
Dropping the parameter is −5 lines across the four sites and the definition. `index.html`'s
`mark.hl.stale` rule is the unreachable other end and belongs to `ui-panels`,
which noted it independently on 2026-08-15 — the two want one commit.

### F3. `staleRail` is cleared twice on every repaint

`clearHighlights` clears it (2050) and `applyHighlights` clears it again (2100).
`clearHighlights`' sole caller (2067) calls `applyHighlights` on the next line,
and the second clear is the load-bearing one — the render path reaches
`applyHighlights` without the first. The line in `clearHighlights` is the
removable one. One line; worth taking only alongside F4, which touches the same
rail.

### F4. Three copies of "the open document went away", still diverging

Raised 2026-08-11; half of it has since been fixed and half has not.
`doDeleteFile` (4492) now goes through `showContentMessage`, so the block record
is dropped correctly — but it still nulls `currentFile` *before* the watcher's
`file-removed` arrives, so that listener's `path === currentFile` test compares
against null and its branch, which is the one carrying `refreshOutline()` and
`resetFind()` (5906–5907), never runs. Reader-visible when the deleted file is
the one on screen: an open outline card keeps listing the deleted document's
headings, and an open find bar keeps a count and a live `n` over an empty pane.
`repo-changed` (5946–5950) is a third copy again, with `refreshOutline` and
without the rail-clear ordering of the other two.

Proposed: one `clearOpenDocument(message)` helper beside `renderCurrent`, doing
the four things in one place — `currentFile = null`, `showContentMessage`,
`refreshOutline()`, `resetFind()` — used by all three. ≈ −6 lines and one class
of divergence. `repo-changed` already calls `resetFind()` earlier for its own
reason (a pattern must not survive a repo swap); a second call is a no-op, worth
confirming before landing.

Needs checking by eye: delete the open file with the outline card open, and again
with the find bar open on a matching pattern.

### F5 (downgraded from 2026-08-11). The second flatten is now only a second walk

The old F2 proposed unifying `locateInNodes` and `placeAcrossNodes` so a repaint
with any cross-node quote stops flattening twice. Half of that has landed since:
`scanTextNodes(roots, doc ? doc.text : null)` (2233) reuses the previous text and
skips the join, which was most of the cost. What remains is a second TreeWalker
pass over the same roots — real but small, and the unification carries a
behaviour change (which occurrence a doubled quote paints) that only a
hand-check can approve. Recommendation: leave it. Recorded so the next sweep does
not re-derive it.

## Read and found clean

The glide and position-frame block (3286–3600) — unchanged since the last sweep
and still carrying the reason for every constant in it. `stagedFullWrite`'s
supersede guard, the `writeGen` protocol, `renderCurrent`'s two-command ordering
and its scroll-restore guard, the image/zoom passes, `decorateCodeBlocks`,
`scrollToFragment`, `selectionContext`, `overlappingIds` and the whole resize
mode: nothing dead, nothing duplicated, and every non-obvious line carries its
"because".

## Adjacent, out of area

- `perf/harness/ui-check.mjs` — the delta assertion that would have caught F0
  (see above). `perf-harness`'s, or F0's own commit.
- `.claude/skills/upkeep/SKILL.md` also says 5,700 lines (D5).
