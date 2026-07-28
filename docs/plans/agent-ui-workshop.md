# Agent + UI workshop

**Status: decisions mostly settled, ready to build.** This file is written to be
read *in dreamd* and argued with: highlight the sentence you disagree with,
annotate it, stack it, send it. Every claim is deliberately one sentence on one
line so a highlight lands on exactly one idea and the anchor is unambiguous.
Fenced code blocks are avoided on purpose — highlights inside a syntect-rendered
block never paint, so anything worth arguing with is kept in prose or inline code.

The scope is the reading-to-agent loop, not the renderer. Everything here is UI
and session behaviour: build it with agility, no perf gate. `/perf-quick` after
the pass, not before each edit.

Second pass folded in on 2026-07-28. One of the twelve original asks turned out
to be a real bug and is already fixed; two are cancelled; the rest are decided.

---

## 0. How to use this document

Section 1 is the decisions, gathered so nothing downstream has to be re-derived.

Sections 2–4 are the asks, one heading each. Every ask has **Today** (what the
code does now, with file references), **Decided** (what gets built), and **Open**
where anything genuinely remains.

Section 5 is the build order and the test plan.

Section 6 is what was cancelled, kept so it is not re-proposed by accident.

Highlight an **Open** line and annotate it with your answer. There are eight left.

---

## 1. Decided

**D1. The pane gets dressed, not rebuilt.** Restyled xterm over the existing pty
ships this pass. The native renderer — driving Claude Code headlessly and painting
turns as app elements — gets its own plan file rather than being bolted on. The
pty stays behind the same `Sink` closure boundary it already has, so that plan can
replace the frontend without touching the backend.

**D2. Ctrl+Enter is one verb: hand the stack to the agent.** It opens the pane if
closed, cold-starts Claude Code if it is not running, and submits the stack. The
embedded pane is the only destination on this key.

**D3. A send wipes the stack and leaves pending highlights.** The sent ids clear
from the stack, every highlight stays, and each gains a pending glyph in the stale
rail that clears when the agent sets `resolved` through MCP. No decay: a pending
mark stays pending until answered or cleared by hand.

**D4. Age is desaturation, state is a glyph, origin is the one hue.** A
prior-session mark is the same hue, much less saturated. Stale and pending live in
the margin rail as glyphs, not colours. `Origin::Agent` gets the one distinct hue.
Age is binary — this session or not — which means a transient `prior` flag, no
timestamp, no schema change.

**D5. Pending is its own field.** `sent_at: Option<u64>` sits alongside `state`, so
a passage that goes stale while out with the agent shows both glyphs honestly
rather than picking one and picking wrong. It is persisted, so pending survives a
restart. `Highlight` is `#[serde(default)]` container-wide, so an older marks file
loads unchanged.

**D6. The tmux path lives on as a hidden debug keybind.** `send.rs`, `tmux_target`
and `tmux_autodetect` keep their tests and gain a caller that is not in the default
keymap and not in the settings panel — reachable when you want to compare the two
paths, invisible otherwise. This is what keeps it out of dead-code territory
without pretending it is a feature.

**D7. Answers-then-edits is a prompt instruction.** One line in the assembled
prompt: answer every question in order, then act. No state machine, no second turn,
no session restart. It can be ignored, and that is the accepted cost.

**D8. An agent mark never steals your scroll.** It raises a marker in the stale
rail, in the D4 hue, and you jump to it when you are ready.

**D9. Answers live in the panel.** The margin glyph clears and the resolution is
readable from the mark; `Resolution.note` already stores it, so the reader gets no
new render path. Inline answers stay unbuilt.

**D10. Two verbs, two keys.** Ctrl+Enter runs the stack; `toggle_pane` (Ctrl+T)
shows the agent. On an empty stack Ctrl+Enter simply opens the pane — it never
scolds you and never does nothing.

**D11. A mid-turn send queues, detected from the output stream.** dreamd watches
the pty output for the shape of an idle prompt and submits when it sees one. Two
queued sends are two turns, not one merged submission.

**D12. Escape closes the pane, in every mode.** The Claude Code session keeps
running behind it — closing is hiding, exactly as `closePane` already works — and
only an explicit Ctrl+C to the child stops the process.

**D13. Re-annotating revives a faded highlight.** Editing or adding to a
prior-session mark's annotation clears its `prior` flag and returns it to full
saturation. The fade means "untouched this sitting", and annotating is touching.

**D14. Re-annotating a pending mark mints a second question.** It does not clear
`sent_at` and does not replace the question already out with the agent — it stacks
a new pair about the same passage.

**D15. The fade ratio is declared per mode by the palette.** A single ratio cannot
be right for both a dark and a light member of the same family, and every palette
is a family. Past highlights should read as *very* subtle.

**D16. A send has an undo window.** For a few seconds after Ctrl+Enter the send can
be reversed and the state returns to exactly as though nothing was sent — the same
shape Claude Code itself uses.

**D17. A partial send wipes only what it sent.** `Send selected` clears the
selected ids and leaves the rest of the stack standing.

**D18. The terminal grid stays monospace.** App-styled chrome — header, padding,
background, scrollbars, selection — but the grid keeps a monospace face, because
Claude Code draws box-drawing characters that only align in one.

**D19. Minimal visual changes, whichever is cheaper.** Don't fight the TUI. Take
the changes with the largest visual payoff per line, ship them, and iterate from
there rather than designing the perfect masked terminal up front.

**D20. Dragging the tree past its minimum collapses it.** The drag and the
collapse keybind become the same gesture at the extreme. There is also a
reasonable maximum — the tree never takes over the window.

**D21. The outline overlay fades when the pointer is away, tracks nothing, and
auto-closes when unused.** Its whole job is one jump; it should not become
persistent chrome.

**D22. The root field is click-to-edit, any path.** `~` expansion, tab-completion
against directories, basename when unfocused and full path when focused, an error
state that leaves you in the current root. The target need not be a git repo. No
history, no dropdown, nothing new written to `~/.config/dreamd/`.

**D23. Testing stays light and stays honest.** No frontend module split. Rust unit
tests cover the pure logic; `ui-check.mjs` grows assertions aimed at *"the UI did
not break"* rather than at appearance. Everything visual is your hand check.

---

## 2. Corrections, and one real bug

### 2.1 Stack persistence — closed, nothing to do

`marks_file::save` writes the ordered stack alongside the highlights and
`marks_file::admit` restores it, pinned by `a_round_trip_preserves_stack_order`.
The only case where it legitimately does not persist is a secondary dreamd, which
holds no socket lock and therefore writes no marks.

Closed. Not a bug, no further work.

### 2.2 The titlebar drag — was a real bug, now fixed

**Today, before the fix:** `#titlebar` carried `data-tauri-drag-region` and the
window used `titleBarStyle: "Overlay"` with `hiddenTitle: true`, so every visible
sign said dragging should work. It did not.

**The cause was the ACL, not the markup.** `src-tauri/capabilities/default.json`
granted `core:default`, which resolves through `core:window:default` to 28
permissions — and `allow-start-dragging` is not one of them. wry's injected handler
fired on mousedown and issued the `start_dragging` IPC call, and the ACL denied it
silently. No console error, nothing wrong in the HTML to find.

The tell was that `allow-internal-toggle-maximize` *is* in the default set, so
double-clicking the titlebar to maximize kept working while dragging did nothing.

**Fixed** by adding `core:window:allow-start-dragging` to the capability. Verified
only as far as this environment allows: the build is clean and the regenerated
`gen/schemas/capabilities.json` carries the permission. Whether the window
actually drags is your hand check.

**Still to build:** view mode hides `#titlebar` outright, which leaves a full-bleed
reading window with nothing to grab. A persistent ~10px invisible drag strip at the
top, surviving view mode, means the window is never un-draggable.

**Open:** should that strip also be a hover target that temporarily reveals the
titlebar in view mode, or does revealing chrome on hover defeat view mode?

### 2.3 Ctrl+Enter's old binding

**Today:** `keymap.send_stack` is `Ctrl+Enter` and routes through `send.rs` to a
tmux target, falling back to the clipboard.

**Decided (D2, D6):** the key now hands the stack to the embedded pane, and the
tmux path moves to a hidden keybind for your own debugging — absent from the
default keymap and from the settings panel, so it neither rots nor advertises
itself.

**Open:** what combo? It wants to be something the settings panel will never flag
as a clash and your fingers will never find by accident.

---

## 3. Session, colour and stack semantics

### 3.1 Prior-session highlights read as faded

**Today:** a `Highlight` carries `state` and `origin` but nothing recording when or
in which session it was made, and the id is minted from a per-process seed, so age
cannot be recovered from it.

**Decided (D4):** a transient `prior` flag set by `marks_file::admit` on everything
it loads from disk, never persisted. A mark read off disk is by definition from a
prior session. No schema change, no clock, and it stays correct across a
save-then-reload because a reload *is* a new session.

**Decided (D15):** the desaturation ratio is a per-mode value the palette declares,
because one number cannot serve both members of a family. Past highlights should be
*very* subtle — present enough to locate, quiet enough to read past.

**Decided (D13):** re-annotating clears `prior` and the mark returns to full
saturation.

**Open:** the ratio is a new palette variable, and every bundled palette plus every
user file needs a sensible answer when it does not declare one. Is the fallback a
fixed opacity multiplier applied in `theme.css`, so an undeclared palette still
fades rather than showing prior marks at full strength?

### 3.2 Sending wipes the stack

**Today:** `send_stack` reads the pairs and hands them off; the stack is untouched,
so the same questions can be sent twice and the badge never drops.

The store already has the vocabulary: `Resolution` exists on `Highlight` with the
documented semantic *"the highlight stays, the stack entry goes."*

**Decided (D3, D5, D17):** a send clears exactly the ids it sent, keeps every
highlight, and sets `sent_at` on each. `Send selected` clears only the selection.
`sent_at` is persisted, so pending survives a restart.

**Decided (D16):** a few-second undo window during which the send can be reversed
and the state returns to exactly as though nothing had been sent.

**Decided (D14):** re-annotating a pending mark mints a second question rather than
clearing `sent_at`.

**Open:** D16's undo has to reverse two different things depending on when you hit
it — before the text reaches the pty it is a pure state rollback, after it is a
message already in Claude Code's input. Does the undo window simply *delay* the
submission by its own length, so there is never anything to take back?

**Open:** pending survives a restart, which means a stack you sent before closing
comes back as pending marks with no agent left to answer them. Does launching with
pending marks say something, or is a quiet margin glyph enough?

---

## 4. Layout, chrome and flow

### 4.1 The pane stops looking like a terminal

**Today:** `#pty-pane` is xterm.js over a real pty running a login shell that
`exec claude`s, docked under the reading pane, built lazily on first open, output
base64-encoded across IPC because a 4 KiB read splits multi-byte characters.

**Decided (D1, D18, D19):** restyle the chrome — header, padding, background,
scrollbars, selection — and leave the grid monospace. Take the cheapest changes
with the largest visual payoff, ship, and iterate. Do not fight the TUI.

**Decided (D12):** Escape closes the pane in every mode. The session keeps running
behind it; only an explicit Ctrl+C to the child stops the process.

**A concern, stated once, then built as decided.** In Claude Code, Escape is how
you interrupt a running turn. Claiming it in dreamd means that while the pane has
focus you can no longer stop a turn you regret, and Ctrl+C in Claude Code is closer
to "exit" than to "cancel that thought". The cost of D12 is a lost interrupt, not
just a changed keybind.

**Open:** is a double-Escape acceptable as the compromise — the first Escape goes
to Claude Code as an interrupt, a second within a short window closes the pane? It
preserves the interrupt and still gives Escape a predictable meaning.

### 4.2 The pane docks right or bottom

**Today:** bottom only, a fixed percentage of window height, hidden with
`display: none` so the session survives a close.

**Decided:** an `agent.position` config key with `bottom` (default) and `right`.

Popup / explode mode is **cancelled** — see section 6.

**Open:** when the pane is docked right, does the outline overlay move out of its
way, or are they allowed to overlap?

### 4.3 The file tree is draggable

**Today:** `#sidebar` is a fixed width, collapsed and expanded by keybind with no
intermediate state.

**Decided (D20):** a drag handle on the right border, a minimum around 140px below
which the drag snaps to collapsed, a reasonable maximum, and the width persisted to
`config.toml` so it survives a restart.

### 4.4 The contents panel hovers

**Today:** `#outline-panel` is a docked side panel sharing visual language with the
stack panel.

**Decided (D21):** float it top-right over the reading pane, narrow by design,
overlapping the text on a narrow window rather than reflowing the reader. It fades
when the pointer is away, tracks no scroll position, dismisses on a heading click,
and auto-closes when unused.

**Open:** "unused" needs a number. Is auto-close a timeout since the last pointer
movement over it, or does any scroll in the reader dismiss it?

### 4.5 The repo path becomes an input

**Today:** `#repo-name` is a text span showing the basename. The backend is fully
built: `adopt_root` swaps config, re-walks, re-arms the watcher, flushes marks, and
retires and re-binds the MCP socket — and compiles on both platforms on purpose.

**Decided (D22):** click-to-edit, `~` expansion, tab-completion against
directories, basename unfocused and full path focused, an error state that leaves
you in the current root, any path rather than only a git repo, and no history.

**Note on "any path".** `fs_walk` uses the `ignore` crate, which handles a
non-repo directory fine — it simply has no gitignore to honour. The thing to watch
is that a directory outside any repo has no `.dreamd.toml` semantics to speak of
and a very large one (a home directory, `/`) will walk for a long time.

**Open:** should there be a guard against pointing it at something enormous — a
depth or file-count ceiling with a "this looks big, continue?" — or is that your
problem to not do?

### 4.6 Accept-edits is the default, and the default is learned

**Decided:** an `agent.permission_mode` config key defaulting to accept-edits,
written back when you change mode through dreamd's own control. The write path
exists — the settings panel already rewrites `config.toml` at runtime and `Config`
is behind a mutex for exactly this.

**Consequence of D1:** under a restyled TUI the mode lives inside Claude Code, so
dreamd can launch with a mode but cannot see you change it with Shift+Tab. A
mid-session switch is a temporary override dreamd does not learn from.

**Note on tenet 3.** `PANE_COMMAND` is a `const` and a test pins it, because no
user content may be interpolated into a shell. A mode flag is not user content —
it is one of four fixed strings — so the shape that keeps the tenet intact is a
match over a closed enum yielding one of four literal commands, never a format
string.

### 4.7 The core loop

You read. You highlight a passage and annotate it; it lands on the stack and the
badge counts up. You highlight three more. Then Ctrl+Enter.

The pane opens in your preferred position, cold-starts Claude Code if needed, and
submits the whole stack as one turn — the passages as labelled evidence, your
annotations as the questions.

The stack empties in the same motion, with a few seconds in which you can take it
back. The four highlights stay in the document, each carrying a pending glyph.

Claude answers the questions in order, in prose, then begins editing if you asked
for edits and the mode allows.

As each question is answered the agent resolves its mark and the glyph clears.

**Consequence of D7 plus D9, and it is the weak joint.** dreamd cannot see the
answers — under a restyled TUI it sees pixels. The pending glyph only clears if
the agent *calls an MCP tool* to set `resolved`. So the same prompt instruction
that orders answers before edits must also ask the agent to resolve each mark as
it answers, and if it forgets, your margin fills with pending glyphs for questions
that were in fact answered.

**Open:** does a resolve-by-hand gesture — click the glyph, mark it answered —
need to exist as the backstop? My read is yes, and it is cheap.

### 4.8 Adding to the stack mid-turn

**Decided (D11):** the stack accepts it normally, and Ctrl+Enter during a running
turn queues for submission when the turn ends. Turn-end is detected by a heuristic
over the pty output stream. Two queued sends become two turns.

**Open:** the heuristic is the riskiest thing in this plan, because Claude Code's
idle prompt is a TUI redraw rather than a marker, and it will change under us
without warning. What is the failure mode when it misfires — a stack that submits
into the middle of a turn, or one that never submits at all? The second is
recoverable by pressing the key again; the first is not.

### 4.9 The agent's own marks

`Origin::Agent` exists and the MCP tools can create highlights, so the agent can
point at a passage rather than describe it — the most interesting thing the
architecture already supports and nothing currently uses.

**Decided (D8), taking the cheapest shape:** an agent-created mark paints in the
distinct hue and raises a marker in the stale rail. Nothing else. No titlebar
count, no per-file count in the tree, no cross-file surfacing, no auto-scroll.

Revisit once the loop is real and you can see what you actually want from it.

### 4.10 View mode and the agent

View mode hides all chrome, and `#pty-pane` is explicitly in the list it hides.

**Open:** does opening the agent leave view mode automatically, or does view mode
simply have no agent?

---

## 5. Building it

### 5.1 Order

Nearly independent items, ordered by unblocking value rather than size.

The titlebar ACL fix is done. The drag strip for view mode goes with the chrome
work below.

First the pure Rust logic, because it is testable and the UI reads its output: the
`prior` flag, `sent_at` and the wipe, the undo window's state machine, the two new
config keys, and root validation for the path field.

Then the chrome, which is `ui/` only and hand-verified: tree drag, floating
outline, path input, view-mode drag strip.

Then the pane: `agent.position`, the restyle, and Escape.

Last the flow — Ctrl+Enter's new behaviour, the queue heuristic, resolution
clearing — because it needs everything else to exist first.

**The heuristic in D11 is the one item that can fail on its own terms.** If
watching the output stream turns out to be unreliable, the fallback is a send-now
button that lights up when a stack is queued, and that is a strictly smaller build
than making the heuristic good.

### 5.2 Tests, given the GUI cannot be driven here

The rule from `CLAUDE.md` holds: the GUI is verified by hand, and `ui-check.mjs`
asserts what the page knows, not what it paints. So the plan is to push what can
be tested into Rust and to aim the frontend checks at breakage rather than looks.

In Rust: the `prior` flag surviving `admit` and never being persisted, `sent_at`
set on send and cleared on resolution, a partial send leaving the rest of the stack
standing, re-annotation clearing `prior` but not `sent_at`, both new config keys
layering and writing back like every other key, and root validation accepting a
non-repo directory while still rejecting what `guard` rejects.

In `ui/`, per D23: no module split. `ui-check.mjs` grows assertions that the page
still wires up — every keybind resolves to a handler, the new panels mount and
unmount, the root field round-trips a path through IPC, no handler throws on an
empty stack. That is "the UI did not break", which is what it can honestly check.

Everything visual is your hand check, and should be labelled as one.

### 5.3 Perf

Not gated. `/perf-quick` once at the end of the pass, and again only if the render
path was touched. The one item with plausible perf surface is the floating outline,
because a CSS rule can cost a measurable amount by merely being declared — and that
number would come from Chromium, not WKWebView, so it is relative signal only.

---

## 6. Cancelled, so it is not re-proposed

**Popup / explode mode.** A floating agent panel at ~40% of the window, promotable
and collapsible. Cancelled on 2026-07-28: what was wanted was a panel bound to the
parent window rather than a free-floating one, and the docked right/bottom
preference already covers that. Deferred as an idea with no implementation planned.

**Inline answers in the document.** Answers rendering beside the highlight they
answer rather than in the panel. Not cancelled outright — moved into the native
renderer plan, where turn painting is already under our control.

**Root history.** A dropdown of recently-opened roots. Cancelled: it would be a
fourth thing written to `~/.config/dreamd/` and tenet 2 says that needs its own
decision, which this is — no.
