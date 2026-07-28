# Agent + UI: implementation plan

Companion to `docs/plans/agent-ui-workshop.md`, which holds the decisions (D1–D23)
and the reasoning. This file holds only the build: what changes, in what order, in
which thread, and what "done" means for each.

Written to be read in dreamd, same as the workshop doc — one idea per line, no
fenced code blocks, so a highlight anchors cleanly.

Every thread commits straight to main. `cargo build` must pass before any commit
touching `src-tauri/`. Perf is not gated on this work: one `/perf-quick` at the
very end, and again only if the render path moved.

---

## 0. Thread map

Seven threads. Three of them are Rust-only and testable; three are `ui/`-only and
hand-verified; one is the pane, which is the largest and wants a session to itself.

**T0 — Titlebar drag.** Done. Shipped in this session.

**T1 — Store semantics.** Rust, pure, heavily tested. The `prior` flag, `sent_at`,
wipe-on-send, partial wipe, the re-annotation rules. Everything else reads its
output, so it goes first.

**T2 — Config surface.** Done. `agent.position`, `agent.permission_mode`,
`ui.tree_width`, the hidden tmux keybind. Small, mechanical, unblocks T4 and T5.

**T3 — Palette fade.** CSS plus `theme.rs`. The per-mode desaturation ratio for
prior-session marks, across ten bundled palettes and any user file.

**T4 — Chrome.** `ui/` only. Tree drag, floating outline, root path field,
view-mode drag strip. Four independent pieces, hand-verified.

**T5 — The agent pane.** *Its own thread.* Restyle, `agent.position`, Escape, cold
start. This is the one that needs undivided attention.

**T6 — The flow.** *Its own thread, after T5.* Ctrl+Enter's new behaviour, prompt
assembly, the undo window, the queue heuristic, the resolution loop.

**Dependencies:** T1 blocks T3, T4's rail work and T6. T2 blocks T5 and T6. T5
blocks T6. T4's four pieces block nothing and can be picked up in any gap.

**Recommended split across sessions:** T1+T2 in one sitting, T3+T4 in another, T5
alone, T6 alone. T5 and T6 are where the product is won or lost and where an
interrupted session costs the most.

---

## 1. T1 — Store semantics

The whole point of doing this first is that every visual decision downstream reads
a field this thread adds.

### What changes

`annotations.rs` gains two fields on `Highlight`: `sent_at: Option<u64>`,
persisted, and `prior: bool`, marked `#[serde(skip)]` so it never reaches disk.
The container-wide `#[serde(default)]` means an older marks file loads unchanged
and a newer one loads in an older build.

`Store` gains `mark_sent(&mut self, ids: &[Id])`, which sets `sent_at` on each id
and removes exactly those ids from the stack — not the whole stack, per D17.

`Store::set_annotation` grows two rules. Re-annotating clears `prior`, returning
the mark to full saturation (D13). Re-annotating does *not* clear `sent_at`, and
enqueues a fresh stack entry, so a pending mark re-asked becomes a second question
about the same passage (D14).

`Store::resolve` already exists and already sets `Resolution`; it gains one line to
clear `sent_at`, which is what makes the pending glyph go away.

`marks_file::admit` sets `prior = true` on everything it admits. That is the entire
implementation of "highlighted in a previous session" — no clock, no session id, no
schema field.

`main.rs`'s `send_stack` calls `mark_sent` with the ids it actually sent.

### Tests

These are the reason this thread exists, and they are all pure.

A mark loaded through `admit` has `prior` set; a mark created this session does not.

`prior` never appears in the JSON that `save` writes — assert on the serialized
text, not on a round trip, because a round trip through `admit` would set it again
and hide the bug.

`mark_sent` on a subset leaves the rest of the stack standing and in order.

`mark_sent` sets `sent_at` and the highlight survives — the stack shrinks, the
highlight list does not.

`resolve` clears `sent_at` and leaves the highlight and its quote intact.

`set_annotation` on a `prior` mark clears `prior`.

`set_annotation` on a mark with `sent_at` leaves `sent_at` set and pushes a new
stack entry.

A marks file written before this change loads with `sent_at: None` and no panic.

A marks file written *after* this change loads in a build that predates it — this
one is checked by construction (`#[serde(default)]`, no `deny_unknown_fields`) and
is worth a comment rather than a test.

### Done when

`cargo test --all-features` green, `cargo run --example marks_check` green, and
each new test has been proven to have teeth by breaking the thing it guards and
watching it go red.

---

## 2. T2 — Config surface

### What changes

`config.rs` gains an `[agent]` section with `position` (`bottom` default, `right`)
and `permission_mode` (accept-edits default), and a `ui.tree_width` integer.

All three go through the existing machinery — `deep_merge` on raw tables,
`patch_global`, `set_global_key` — so nothing new is invented and the settings panel
and the `dreamd config` subcommand both get them for free.

`Keymap` gains a `send_stack_tmux` entry that is **not** in the default keymap and
**not** listed by the settings panel: the hidden debug binding of D6. It keeps
`send.rs` alive with a real caller rather than a dead-code allow.

`.dreamd.toml` is repo content and therefore untrusted. `position` and `tree_width`
are harmless for a repo to set. `permission_mode` is **not** — a repo that could
set accept-edits would be choosing how much your agent is allowed to write. Add it
to the local-file denylist beside `theme_css`.

### Tests

The three keys layer global-under-local the way every other key does.

A local `.dreamd.toml` that sets `agent.permission_mode` is ignored, and says so —
same shape as the existing `theme_css` test.

A local file that mentions only `[agent]` does not wipe a global `[keymap]`, which
is the regression `deep_merge` exists to prevent and is worth re-asserting for a new
section.

`tree_width` round-trips through `set_global_key` and comes back clamped.

`send_stack_tmux` is absent from `default_keymap()`.

### Done when

`cargo test --all-features` green and `cargo run --example config_check` green.

### Done (2026-07-28)

Shipped as written, with three notes for the threads downstream.

The local-file denylist moved out of `Config::load` into a pure
`strip_untrusted(&mut Table)` returning `(key, why)` pairs, so both refusals are
unit-testable rather than reachable only through `config_check`'s sandboxed
`XDG_CONFIG_HOME`.

`ui.tree_width` is **clamped on deserialize**, not validated: 140–600, default
260 (the sidebar's old fixed width). Out of range is the nearest usable tree, not
a rejected file that takes every other key down with it. A *negative* width is
still rejected outright, because it never reaches `u32`. The file keeps whatever
was written; every reader sees the clamp, so T4 can persist a drag without
round-tripping through a validator.

`keymap.send_stack_tmux` is `Option<String>`, `None`, and
`skip_serializing_if` — so "Reset all shortcuts", which patches the global file
with `default_keymap()`, cannot clear a binding set by hand. Nothing dispatches
it yet: the frontend half belongs to T6, where Ctrl+Enter stops being the tmux
path.

---

## 3. T3 — Palette fade

### What changes

`ui/theme.css` gains a `--hl-prior` treatment applied to `mark.hl[data-prior]`.

Each bundled palette declares the ratio per mode, inside its
`:root[data-mode="light"]` and `:root[data-mode="dark"]` blocks, because one number
cannot serve both members of a family (D15).

`theme.css` carries a fallback so a palette that declares nothing still fades —
otherwise every pre-family user file would show prior marks at full strength, which
is the loud failure rather than the quiet one.

`app.js` sets `data-prior` on the `mark` element from the highlight's `prior` field.
`readCssVar` / `modeSlice` already mirror `theme.rs`'s mode handling; if the ratio
needs reading on the Rust side it goes through `custom_property` with a `Scheme`,
same as `--bg` and `--syntax-theme`.

### Tests

`theme.rs`'s existing CSS-parser tests extend to the new variable: it is found in
the right mode slice, and the other appearance's value does not leak.

A palette declaring no ratio falls back rather than returning `None` into a style
string.

`cargo run --example theme_check` covers all ten bundled palettes for presence.

### Hand check

This is the one item where "very subtle" is a judgement, not an assertion. Look at
it in at least one light and one dark palette before committing the numbers.

---

## 4. T4 — Chrome

Four independent pieces, `ui/` only, none blocking the others.

### 4a. Tree drag

A drag handle on `#sidebar`'s right border. Minimum ~140px; dragging below it snaps
to collapsed, which makes the drag and `toggle_tree` the same gesture at the
extreme (D20). A reasonable maximum so the tree never eats the window.

Width persists to `ui.tree_width` from T2, debounced so a drag does not write the
config file forty times.

### 4b. Floating outline

`#outline-panel` moves from docked to floating top-right over the reading pane,
narrow by design, allowed to overlap text on a narrow window (D21).

It fades when the pointer is away, tracks no scroll position, dismisses on a
heading click, and auto-closes when unused.

The auto-close rule needs picking: a timeout since last pointer movement over it,
or dismissal on any reader scroll. The second is simpler and probably right.

### 4c. Root path field

`#repo-name` becomes click-to-edit: basename unfocused, full path focused, `~`
expansion, tab-completion against directories, and an error state that leaves you in
the current root (D22).

Submitting calls `adopt_root`, which is already fully built — config swap, re-walk,
watcher re-arm, marks flush, socket retire and re-bind.

Any path, not only a git repo. `fs_walk`'s `ignore` walker handles a non-repo
directory fine; it simply has no gitignore to honour.

The completion list comes from a new command returning directory children, and that
command must not become a filesystem browser for arbitrary content — it returns
directory names only.

### 4d. View-mode drag strip

A persistent ~10px invisible `data-tauri-drag-region` strip at the top of the
window, surviving `body.view-mode`, so the window is never un-draggable.

It sits above the titlebar in the layout and below it in attention: no hover
affordance, no background, nothing to notice.

### Tests

Per D23 there is no frontend module split, so these are `ui-check.mjs` assertions
aimed at breakage rather than looks.

The outline panel mounts and unmounts without throwing.

The root field round-trips a path through IPC and an invalid path leaves the
displayed root unchanged.

Every keymap entry still resolves to a handler after the new elements exist.

The tree width clamps at both ends.

### Hand check

All four are visual. In particular the drag strip cannot be asserted at all — the
only proof is that a view-mode window moves when you drag its top edge.

### Done (2026-07-28)

Shipped as written, with four notes.

The auto-close rule is dismissal on any reader scroll, not a timeout — plus an
explicit `closeOutline()` on a heading click, because a jump to the heading
already on screen scrolls by nothing at all and would otherwise leave the panel
standing.

The completion command is `complete_directories`, and the rules it enforces
live in a new `rootfield` module rather than in `main.rs`: absolute paths only
(so nothing resolves against a Finder launch's `/` cwd), an existing directory
only, directory *names* and never paths or file names, dot-directories only
when the typed prefix asks for one, and a 200-entry cap. `set_root` is the
second command and does nothing but decide whether the line is a path before
handing it to `adopt_root`.

The tree width reaches the frontend through a new `get_ui` command, issued in
the same round trip as `get_keymap` — `get_settings` carries `[ui]` too, but it
walks the themes directory and this is the boot path.

The drag strip is `z-index: 0` with `#titlebar` at `z-index: 1`, rather than
`pointer-events` juggling: while there is a titlebar it covers the strip and
takes the mousedown, and both carry `data-tauri-drag-region` so the two cases
are the same gesture.

---

## 5. T5 — The agent pane

**Its own thread.** Everything before this is small and mechanical; this is where
the app either starts feeling like one thing or does not.

### What changes

`agent.position` from T2 gets an implementation: `#pty-pane` docks bottom (today's
behaviour) or right, with the fit-addon resize path already in place handling the
new geometry.

The restyle, per D18 and D19: app-styled header, padding, background, scrollbars and
selection; the grid stays monospace because Claude Code's box-drawing only aligns in
one. Take the cheapest changes with the largest visual payoff and stop.

Escape closes the pane in every mode, with the session left running behind it —
`closePane` already hides rather than kills, so this is a keybind change, not a
lifecycle change (D12).

Claiming Escape means editing `attachCustomKeyEventHandler` in `buildTerminal`,
which currently claims exactly one key and documents why. It becomes two, and the
comment has to say what it costs.

`agent.permission_mode` reaches the child through a match over a closed enum of
four literal commands. `PANE_COMMAND` stays a `const` per tenet 3 — the mode is one
of four fixed strings, never a format string, and the existing test that pins the
command extends to pin all four.

A mode control in the pane header that restarts the session with the new mode and
writes the preference back.

### The thing to decide while building it

Escape is Claude Code's interrupt. Claiming it costs you the ability to stop a turn
you regret while the pane has focus, and Ctrl+C is closer to "exit" than "cancel".

The workshop doc floats double-Escape as the compromise — first goes to the child,
a second within a short window closes the pane. Decide it with the pane in front of
you, because it is a feel question.

### Tests

`pty`'s existing tests extend to the four mode commands, still driven with
`/bin/sh` and never with `PANE_COMMAND`, so `cargo test` still cannot start a Claude
Code session.

`ui-check.mjs` asserts the pane mounts in both positions and that its close path
leaves the session flag set.

### Hand check

Everything about how it looks, and specifically: whether a reading-coloured
terminal and a red/green diff disagree badly enough to matter.

---

## 6. T6 — The flow

**Its own thread, after T5.** This is the part that needs everything else to exist.

### What changes

Ctrl+Enter becomes one verb (D2): open the pane if closed, cold-start if needed,
submit the stack. On an empty stack it simply opens the pane and does nothing else
(D10).

Prompt assembly moves from `send.rs`'s tmux shape to the pane. The stacked quotes
keep going through `untrusted::delimit` per tenet 6, and dreamd's own instruction —
answer every question in order, then act, and resolve each mark as you answer it —
goes *outside* the sentinel, above the evidence, so a document cannot appear to be
issuing it.

The undo window (D16): rather than sending and then taking it back, the submission
is *delayed* by the window's length, so there is never anything to retract. The
stack shows the pending send and one gesture cancels it.

The queue heuristic (D11): watch the pty output stream for the shape of an idle
prompt, submit when one is seen, and treat two queued sends as two turns.

The resolution loop: `marks-changed` already fires from the MCP layer only, and
`Store::resolve` already clears `sent_at` after T1, so the glyph clearing is
existing plumbing — provided the agent actually calls the tool.

A resolve-by-hand gesture on the glyph, as the backstop for when it does not.

### The risk, named up front

The heuristic is the least durable thing in this plan. Claude Code's idle prompt is
a TUI redraw, not a marker, and it will change under us without warning.

Decide the failure mode deliberately: a queue that never fires is recoverable by
pressing the key again, and a queue that fires into the middle of a turn is not. Bias
the heuristic toward never firing.

The fallback, if it proves unreliable, is a send-now button that lights when a stack
is queued — a strictly smaller build than making the heuristic good, and worth
reaching for early rather than late.

### Tests

Prompt assembly is pure and gets real coverage: the instruction sits outside the
sentinel, the evidence inside it, and a document containing something that looks
like a delimiter cannot break out.

The undo window's state machine is pure: queued, cancelled, submitted, and no path
that submits twice.

`ui-check.mjs` asserts Ctrl+Enter on an empty stack opens the pane and throws
nothing.

### Hand check

The whole loop, end to end, on a real document with a real stack. That is the only
check that means anything here.

---

## 7. Closing the pass

After T6, one `/perf-quick`, and `/perf-pass` only if the render path moved.

Then `/wrap-up`, which commits, prepends to `docs/session-log.md` and refreshes
`engies/project.md` — and this pass changes the project story enough to warrant it,
because the tmux send loop stops being the product's centre.

`docs/todo.md` gets the items this plan does not cover, and the workshop doc's
remaining open questions stay where they are rather than being copied here.
