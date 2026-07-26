# Stack panel: visual polish + push/pop motion

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

## Open question

What should "pop" look like given removal isn't always from the top of the
list (per your note — just borrowing the motion, not the literal mechanic)?
Options: the removed card slides out sideways and the rest resettle, a fade
+ collapse in place, or a snap-toward-the-panel-edge motion regardless of
list position. Worth picking one metaphor and applying it consistently
rather than literally simulating a LIFO stack visually.
