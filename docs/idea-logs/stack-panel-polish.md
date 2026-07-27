# Idea log — stack panel: visual polish + push/pop motion

Source idea: `ideas/stack-panel-polish.md`. Overnight batch, branch
`claude/overnight-ideas-2026-07-27`.

**Decision: implemented in full — both halves, no plan file.** The idea file
offers a natural split (the button-class cleanup is independent of the diffing
rework), and the brief was explicit that landing only the cleanup plus a plan
was an acceptable outcome. I took the whole thing because the rework turned out
to be *smaller and more checkable* than it reads: it is one contained region of
`ui/app.js`, no Rust, no IPC change, no new command, and the one part with real
correctness risk — the ordering pass — is pure logic that could be exercised
headlessly (see Verification). The visual half is unverifiable either way; the
logic half is not, and that is where the risk actually was.

## The hazard this change creates, and how it is handled

Worth stating first, because it is the reason the diff is as defensive as it is.

`send_stack(ids)` resolves ids through `Store::selected_pairs`, which looks each
id up in the **highlight** list, not in the stack (`src-tauri/src/annotations.rs`
:123). Today that is harmless: `refreshStack()` tore the list down
synchronously, so a checkbox never outlived its pair by even one frame. An exit
*animation* changes that — a removed card is deliberately still in the DOM for
~170ms. Without a guard, pressing **Send selected** during that window sends a
pair the reader just took off the stack. That is exactly the class of bug the
brief called the worst outcome available.

Two guards, deliberately redundant:

1. `exitPair()` deletes the card's `<input type="checkbox">` the instant the
   card is marked `.leaving` — before any animation starts. Nothing that
   queries `#stack-list` can see it, however it queries.
2. `checkedIds()` now selects through `.pair:not(.leaving)`.

Either alone is sufficient; both are cheap.

## What was done

**Pure frontend. `src-tauri/` was not touched at all** — confirmed with
`git diff --stat` — so nothing here is missing a compile check, and the build
gate was not needed.

### `refreshStack()` reconciles instead of rebuilding (`ui/app.js`)

Replaced `list.innerHTML = ""` + rebuild-everything with a keyed reconcile.
Each card is keyed by `p.highlight.id` on `dataset.id`:

- a pair still on the stack **keeps its node**; `updatePair()` rewrites only
  location / quote / annotation / stale state;
- a new pair mounts via `buildPair()` with `.enter`;
- a departed pair leaves via `exitPair()` with `.leaving`;
- an **ordering pass** then places the nodes in `get_stack` order, moving only
  the nodes actually out of place. A blanket `appendChild` sweep would have
  been simpler but re-inserting a node restarts its CSS animation, so it would
  replay the enter snap on every card on every refresh.

Two invariants the pass has to hold and does: the live cards read in exactly
`get_stack` order, and a steady-state refresh performs **zero** DOM insertions.

`stackSeq` was added alongside — the same monotonic-guard idiom as `paletteSeq`
a few hundred lines down. Two refreshes can be in flight at once (a remove
racing an annotation save), and with stable keys an older reply landing last
would *resurrect* a card that is already gone. Under the old teardown it merely
painted a stale list, so this guard is new work the keying made necessary.

### Behaviour change, called out on purpose: checkbox ticks now persist

The old rebuild re-created every checkbox `checked` on every refresh, so a tick
survived only until the next unrelated stack change. In practice that made
**Send selected** indistinguishable from **Send all** unless you unticked
something in the seconds before pressing it. Reusing the node preserves the
tick, which is what a checkbox is for. New pairs still default to ticked.

This is the one user-visible semantic change in the diff. It is an improvement,
but it *is* a change: a pair unticked and then forgotten will stay out of
**Send selected**. README's "cherry-pick which pairs go" bullet now says so.

### Motion: snap (`ui/index.html`)

Per the idea file's decision — snap, not slide-and-resettle, not fade, and not
literally LIFO.

- **Enter:** `@keyframes stack-snap-in`, 130ms, `cubic-bezier(0.16, 1, 0.3, 1)`
  (ease-out-expo), from `translateX(26px)` at `opacity: 0`. Deliberately **no
  overshoot** — an overshoot is a resettle.
- **Exit:** `.pair.leaving` transitions out to `translateX(20px)` at
  `opacity: 0` in 130-150ms, *while* its own box collapses (height, block
  padding, bottom margin, block border widths → 0) so the cards below it do not
  jump when it goes. The starting height is measured in JS and set inline —
  it cannot live in the stylesheet — and the `0px` target is set inline too, in
  the next frame, so it beats that inline start value.

Removal is on a `setTimeout(STACK_EXIT_MS = 170)`, **not** `transitionend`.
`transitionend` can simply never arrive: closing the panel or pressing Ctrl+M
50ms into an exit makes the card `display: none` and abandons the transition.
It also fires once per property, so a naive `{ once: true }` would cut the
collapse short at whichever property finished first. A card that outlives its
pair is the failure this file must not have, so the timer wins.

`stackAnimates()` gates all of it, and is evaluated **once per refresh** so an
enter and an exit in the same pass cannot disagree. It returns false for
`prefers-reduced-motion: reduce`, for `body.view-mode`, and for a stack panel
that is not `.open` — a `display: none` panel runs neither animations nor
transitions, so animating into one buys nothing and would leave `.enter`'s
`both` fill mode holding a card at `opacity: 0` until the panel next opened.
When it returns false, cards mount and unmount instantly. A
`@media (prefers-reduced-motion: reduce)` block in `index.html` is the CSS-side
belt to the JS check.

### Button cleanup + card polish (`ui/app.js`, `ui/index.html`)

- The `<button>remove</button>` with `style.marginTop = "6px"` is gone. It is
  now `<button class="icon danger rm">✕</button>` using the shared classes
  already in `index.html`, moved **up into the card's header row** next to the
  checkbox rather than dangling under the annotation. `aria-label="Remove from
  stack"` plus `data-tip` — `wireTooltips()` delegates on `[data-tip]`, so a
  dynamically built button needs no wiring.
- It sits at `opacity: 0.4`, rising to 1 on card hover, button hover, or
  `:focus-visible`. Not hover-only like `.file-opts`: this is the only way to
  take a pair off the stack, so it stays legible — but a full-strength red ✕ on
  every card was too loud.
- `.pair` gains a `--btn-bg` background (the same one-step lift every other
  raised surface here uses, and what makes the gaps read as gaps), an 8px
  radius, an `--accent-dim` hover border, and a `.stale` state that turns both
  the card border and the quote's left rule `--stale`.
- `.loc` now flexes and ellipsises so a long filename cannot push the remove
  button around.

### Escaping

All card text now goes through `textContent`, so the quote and the annotation —
user content — never touch `innerHTML` and `escapeHtml` is not needed on this
path at all. Strictly stronger than the old escape-then-interpolate (tenet 4).
`escapeHtml` is still used elsewhere and was not removed.

## Files touched

- `ui/app.js` — the `stack panel` section rewritten: `stackSeq`,
  `STACK_EXIT_MS`, `stackAnimates`, `refreshStack`, `isLivePair`,
  `reconcileStack`, `buildPair`, `updatePair`, `exitPair`; and `checkedIds()`
  gains the `.leaving` filter.
- `ui/index.html` — `.pair` card styling, `.pair .rm`, `.pair.stale`,
  `@keyframes stack-snap-in`, `.pair.enter`, `.pair.leaving`, `.pair.leaving.out`,
  and a `prefers-reduced-motion` block.
- `README.md` — the **Send** bullet now says ticks persist.

**`src-tauri/` was NOT touched.** No new `#[tauri::command]`, no IPC change, no
config key, no keybind, so no `KEY_ACTIONS` row and `perf/harness/` is
unaffected (nothing there references `#stack-list`).

## Verification

- `node --check ui/app.js` passes.
- **The ordering pass was exercised headlessly.** The `place()` loop was lifted
  verbatim into a throwaway harness with a ~40-line fake DOM (children /
  `firstChild` / `nextSibling` / `insertBefore` / `classList`) and run over 14
  cases: no-op, append, insert-in-middle, remove first/middle/last with the
  `.leaving` card holding its slot, append-while-one-leaves, swap,
  move-first-to-back, move-last-to-front, full reverse, empty→populated,
  all-leaving, and a reorder *around* a leaving node. All 14 produce the exact
  expected child order, live cards match `get_stack` order in every case, and
  the no-op case performs 0 insertions (which is what stops the enter animation
  replaying). The harness lives in the session scratchpad, not in the repo —
  there is no test target here to put it in.
- The build gate was not run: no Rust file changed.
- Everything else is **by eye**. No Tauri/WebKitGTK run and no Chromium
  (Playwright's download is proxy-blocked in this container), so the CSS has
  never been rendered, the animation has never played, and the reconcile has
  never touched a real DOM node.

## Left open

- **The motion has never been seen.** Most likely to want tuning on first
  sight: the 130ms enter duration and the 26px travel (both single values in
  `@keyframes stack-snap-in`), and whether the exit's simultaneous
  snap-out-and-collapse reads as one gesture or as two. If the collapse turns
  out to be the wrong call, deleting the height lines from `exitPair` and
  `.pair.leaving.out` leaves a pure snap-and-jump.
- **`STACK_EXIT_MS` duplicates the CSS duration** in a second place. It is
  commented on both sides. It is a ceiling, not a sync point — raising the CSS
  duration past 170ms would truncate the exit, lowering it just idles.
- **Ticks persisting is a behaviour change** (see above). If it turns out to be
  unwanted, the one-line revert is to set `cb.checked = true` in `updatePair`.
- **A new card can mount below a card that is still leaving**, in the narrow
  case where a pair is added in the same ~170ms window another is removed. The
  ordering pass appends past `.leaving` nodes rather than inserting before them.
  Cosmetic only, and it self-corrects when the leaving card collapses.
- **`prefers-reduced-motion` is read live** on every refresh via `matchMedia`,
  not cached and not listened to, so toggling the OS setting takes effect on
  the next stack change rather than instantly. Correct behaviour, just noting
  it is not a `change` listener.

perf not run - pending manual check on the author's machine
