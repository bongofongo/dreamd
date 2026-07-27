# Stack panel: visual polish + push/pop motion

**Status: done — shipped in e8e7043 (2026-07-26).** Both decisions below were
built as written: `refreshStack()` reconciles against the previous list keyed
by highlight `id` instead of wiping `innerHTML`, which is what gave the cards a
stable identity to animate against, and pairs snap in and out toward the panel
edge. The `style.marginTop` hack is gone in favour of the shared button classes.

Better-looking stack entries, plus animation that reads as "pushing" or
"popping" (not necessarily literal top-of-stack only — just that motion
language).

## Current state

`refreshStack()` (`ui/app.js:561`) wipes `#stack-list.innerHTML` and rebuilds
every entry from scratch on every change — there's no per-item identity
across renders, so there's currently no hook an animation could attach to.
That's the first thing that has to change, before any CSS transition: diff
against the previous list and only mount/unmount the item that actually
changed, instead of a full teardown.

Buttons are minimal today: a plain `<button>remove</button>` with an inline
`style.marginTop` hack (`ui/app.js:584-587`), not using the shared button
classes already defined in `ui/index.html` (`button.icon`, `button.danger`).
Easy first win independent of the animation work.

## Decisions

- **Motion: snap.** Push snaps a new card in; pop snaps the removed card out
  — toward the panel edge, not a slide-and-resettle or fade. Applies
  regardless of the item's position in the list (removal isn't literally
  always top-of-stack, per the original note — the snap is borrowed motion
  language, not a literal LIFO animation).
- **Architecture change confirmed.** `refreshStack()`'s full teardown/rebuild
  has to go before the animation can exist at all — diff against the
  previous list, mount/unmount only the item(s) that actually changed, and
  give each rendered card a stable identity (the highlight `id` already on
  each pair) to animate against.
