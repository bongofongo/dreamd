# Session log

## 2026-07-30 — the agent stops being a terminal

The fourth and last step of what "AI-integrated" can mean here. dreamd had an
MCP server (the agent reads the stack) and an embedded pane (the agent lives in
the window), but the pane was *Claude Code's* UI: an xterm.js terminal painting
a TUI, with its own palette, its own typography and a composer dreamd could not
see into. A question asked about typeset prose came back in a 13px monospace
box. The conversation is now dreamd's own, rendered through dreamd's own
markdown pipeline, with a native composer and native permission cards. Native
is the default; the terminal is a documented fallback behind
`agent.surface = "terminal"`.

Six commits, `3681a64` → `b66da19`.

### What happened

1. **Four things were measured against the installed `claude` 2.1.220 before a
   line was written**, and one of them changed the design.

   - Multi-turn over stdin keeps one `session_id` — two `{"type":"user",…}`
     lines, two turns, two `result` messages, no restart. That is the whole
     transport.
   - **`--permission-prompt-tool` no longer exists.** The plan had assumed it
     did. The replacement is a `PreToolUse` hook installed via `--settings`, and
     it is *better*: it fired, blocked for two seconds and denied a `Bash` call
     **while the session ran under `--permission-mode bypassPermissions`**. The
     hook outranks the mode.
   - `{"type":"control_request","request":{"subtype":"interrupt"}}` ends a turn
     without killing the child.
   - **`/model haiku` sent as ordinary user-message text is honoured**, and the
     next turn's `system/init` reports the new model. The plan had named "model
     switching costs a restart natively" as a regression to own up to in the
     release notes. It does not exist; the chips work exactly as before.

2. **`agent/wire.rs` is the only place that knows another program's schema**, and
   it is `serde_json::Value` navigation rather than derived structs on purpose —
   we do not own this format and did not version it. `digest` returns a `Vec`,
   never a `Result`: an unknown `type`, an unknown content block, a line that is
   not JSON each yield nothing. A message kind dreamd has never seen costs a
   ticker row, never the pane. Two committed fixtures carry invented message
   kinds, an invented `stream_event` type and a line of prose to keep that true.

   Text is read **twice** and both readings are used: `text_delta` events paint
   plain so prose visibly moves, then the closing `assistant` block replaces the
   node with markdown-rendered HTML. Rendering mid-stream flickers between block
   types as fences and list markers arrive; waiting for the end makes a long
   answer look like nothing is happening. Tool calls are read only from the
   complete message — `content_block_start` announces an empty `input` and
   dribbles the real one out in fragments.

3. **No shell.** The pane runs `$SHELL -l -i -c "exec claude"` because a `.app`
   from Finder inherits launchd's minimal `PATH`. That reasoning holds; the
   remedy does not have to. `agent::claude::resolve` asks a login shell *where*
   `claude` is, once, and everything after that is `Command::arg`. It matters
   more here than it did there: this launch carries two dreamd-minted paths
   inside a `--settings` JSON document, which through a shell would be a quoting
   problem with a security answer.

4. **The permission gate inverts the old trade.** The six pre-granted tools were
   the whole policy precisely because Claude Code's own prompt lands in a
   terminal nobody is watching. With a card in the window the reader is already
   in, the six become the *fast path* — a tool on the list never reaches the hook
   — and the policy moves out of an argv string into `gate::decide`, a pure
   function with tests. **Deny is the answer to every kind of silence**: a closed
   pane, a retired server, an unparseable payload, an elapsed wait. There is no
   path that fails open, and `agent_spawn` refuses to launch at all if the gate
   cannot be built.

   `dreamd approve` is shaped like `mcp::shim` and inverted where it counts: the
   MCP shim answers `initialize`/`tools/list` locally so a closed dreamd cannot
   blank an agent's tool list for a session — it fails *open* toward usefulness.
   This one has no local answers and fails closed.

5. **The gate socket is per-session, not per-repo.** Routed through the MCP
   socket, a secondary window's agent would raise its cards in the primary's,
   where a reader who never asked the question would be asked to approve it.

6. **Assistant prose goes through `markdown::render_with`** — the same
   pulldown-cmark and syntect the documents go through. One decision, and it buys
   both halves of the point: the answer is typeset in the reader's own palette
   and syntax theme, *and* it inherits tenet 4's escaping rather than needing its
   own, because it is the same string through the same renderer.

7. **Escape interrupts now.** xterm claimed the key and D12 spent it on "close
   the pane", leaving Ctrl+C as the only way to stop a turn — a cost the pane's
   own comments record as deliberate. dreamd owns the keyboard natively, so
   Escape stops a running turn and closes the pane when none is running.

### Mistakes & deviations

- **The plan's permission design was built on a flag that had been removed.**
  Caught before any code, by running `claude --help` instead of trusting the
  research summary that reported the flag as present. The hook that replaced it
  is strictly better, so the plan improved rather than shrank.
- **A `gate-` prefix on a 16-hex socket name would not bind.** macOS caps
  `sun_path` at ~104 bytes and `mcp_check`'s socket already sits at **102**. The
  gate name is now `g<12hex>.sock` — 18 characters against MCP's 21 — so the
  budget question is answered relatively: anywhere MCP binds, the gate binds. A
  test pins the inequality. This was two bytes from being someone else's bug
  report.
- **A `tool_of_answer` stub would have made "Always allow" silently do nothing.**
  Written as a placeholder returning `None`, caught on reread, replaced by
  carrying the tool name on the waiting entry — which is also stricter, since a
  frontend naming a different tool can no longer widen the session's policy.
- **Adding a settings row made an existing harness check pass for the wrong
  reason.** `both chrome toggles listed off macOS` counted rows in the Window
  tab; the third row made it 2 on macOS (titlebar + surface) while the same three
  rows would have failed on Linux. Now asserted by label.
- **The first A/B arm silently produced zero output** — a worktree needs
  `perf/corpus/generated` copied in as well as `node_modules`, or every scenario
  dies with `ENOENT … repos/repo-500`. Exactly the failure mode
  `perf-scripts-fail-silently` warns about, caught by checking the line count.

### State

`cargo build` clean, `cargo clippy --all-targets --all-features -D warnings`
clean. **340 unit tests** pass. All five example harnesses green, including the
new `agent_check` (17 checks), which drives the **real `dreamd approve` binary**
against a **real socket** — the two halves of the gate had never been run against
each other before it. It joins `ci.yml`. `node --test ui/paths.test.mjs` 10/10;
`ui-check.mjs` **260 passed, 0 failed**, including ~40 new checks over the
`agent-event` → DOM seam.

**Both new guards were proven to have teeth rather than merely to be green.**
Forcing `wire::digest` to `unwrap` turned 7 tests red; forcing `gate::decide` to
return `Auto` turned 5 harness checks and 10 unit tests red. Both restored by
hand.

The end-to-end launch path was verified against the real CLI with a throwaway
example since deleted: `resolve()` found `~/.local/bin/claude` through the login
shell, and one turn produced `Ready → Status → two TextDeltas → Text → Turn`,
with the deltas concatenating to the settled block.

**Perf: `perf-pass` read 19 regressed and none of it is this work.** The loud
rows are `chromium.highlight.mixed-512k.spanning.*`, where `applied` went 50 →
214 and `failRatio` went *negative*. That is 2026-07-29's `placeAcrossNodes`
wrapping one `<mark>` per text-node slice sharing an id, so the scenario's
element counter overcounts — a negative fail ratio is the tell that the metric
broke, not the code. Settled by A/B rather than by argument: a worktree at
`1ce7416` against HEAD, 3 reps per arm, put the `spanning.*` rows **flat to −10%**
with the only ≥15% movers being sub-millisecond noise or improvements.
`real.loop.debug-h10.apply_highlights_ms` (+77–92%) and `repaints` (2 → 4) are the
same change's genuine cost — marks that never painted before now paint — but that
arm was **not** A/B'd, so it is reasoning rather than measurement, and it is
recorded here as such. Results in
`perf/results/pass-b66da19-20260730-161849.json`. `perf/baseline.json` untouched.

**Left open, and it is the important one: the GUI itself is unverified.**
`ui-check` asserts what the page knows, not what it paints. Nobody has looked at
the conversation log, the tool ticker or a permission card in a real window —
that needs `cargo tauri dev` and human eyes, and until it happens the visual half
of this session is a claim, not a result.

Also unbuilt, deliberately: `--resume` (the `session_id` is captured and stored
against it), thinking blocks, tool payloads and diffs, and reading the MCP strip
off `system/init`'s `mcp_servers[]` instead of polling.

## 2026-07-29 — the bottom of the window, and what a mark is allowed to say

Four complaints, all from reading with the thing: content at the foot of the
window unreachable, "? still pertinent" on files nobody had edited, a rail
crowded with cards asking to be clicked for nothing, and highlights that
vanished on the next repaint. Every one turned out to be a different bug and all
four are fixed.

### What happened

1. **The file-tree `⋯` menu had no vertical clamp.** `openFileMenu` clamped
   `left` against `window.innerWidth` and set `top` to `r.bottom + 4`
   unconditionally, so for a tree that fills the sidebar every file in the lower
   half opened its menu below the viewport. `#file-menu` is `position: fixed`, so
   nothing was clipping it — it was laid out where nobody could see it. Now
   `.open` goes on *first* (the element is `display: none` until then and measures
   zero), then it flips above the anchor when it will not fit below, both axes
   clamped to a 6px inset. The harness pins it against a synthetic anchor rather
   than a tree row: what changed is the arithmetic, and a fixture tall enough to
   reach the foot of a 900px viewport would be testing the fixture. The old code
   measured **890–970 in a 900px viewport**.

2. **The agent pane's last row was genuinely chopped, and static reading could
   not find it.** `FitAddon.proposeDimensions` divides
   `getComputedStyle(parentElement).height` by the cell height and subtracts only
   `terminal.element`'s padding. Two readings of the spec say that resolved height
   is the content box — Chromium agrees — but **WebKitGTK answers with the border
   box**: a probe in the real window had `#pty-term` at 349.59px with 18px of
   vertical padding reporting `349.59375px`, not the 331.59px it had to give. So
   the addon counted the dock's own padding as usable rows and proposed 20 × 17px
   into a box with room for 19; `#pty-pane`'s `overflow: hidden` took the
   difference off the bottom, which is exactly where Claude Code draws its
   composer. 9.4px with the MCP strip up, and two columns of the same error
   horizontally. The padding moved to `#pty-term .xterm`, where the addon looks
   for it — rows 20→19, cols 86→83, `.xterm` bottom 1036.41→1018.41 against a pane
   ending at 1027.00.

   Found with a temporary `perf.at()` probe encoding the geometry into the phase
   string, which is what `--features perf`'s NDJSON-on-stderr is for. Removed
   before the commit.

3. **`Stale` now means the text was edited, and only that.** Two guaranteed
   false-positive paths, neither involving an agent. `markdown::locate` searches
   raw source for rendered DOM text, so a quote spanning `**bold**` matches no
   tier — tier 3 strips whitespace, not syntax. `add_anchored` deliberately keeps
   such a mark at `(0, 0)`, and `reanchor_file` then flipped exactly those to
   `Stale` on the first re-anchor — which, since every file with marks read off
   disk is re-anchored on first sight, meant a red chip on an untouched file every
   session. Guarded on `line_start > 0`. That makes the implication *exact* rather
   than heuristic: `locate_near` is deterministic in its inputs, so a quote that
   resolved against these bytes resolves again, and past the guard `Stale` ⇔ the
   source changed. Separately the frontend turned a DOM *placement* failure into
   the same chip; both `addStaleChip` calls on that path are gone.

4. **The pending chip and its "Answered" button are retired.** D3 put one card per
   sent mark on the rail with a button whose only effect was to record "dealt
   with" — which the send had already implied, five times over for a
   five-question send. `addPendingChip`, the `.pend-chip` CSS and the
   `resolve_mark` command are all deleted. `sent_at`, `Store::resolve` and the MCP
   `resolve_highlight` stay: they are the agent's record of what it closed and
   `list_highlights` filters on it. The rail's one remaining tenant is genuine
   staleness, which is now rare enough that it is almost always empty.

5. **Placement is a third thing, separate from anchoring, and it spans nodes.**
   The bug behind "highlights aren't re-rendered": `wrapByWalk` and
   `locateInNodes` both look inside a *single* text node, so a cross-markdown
   quote painted once at creation — `wrapRange` on the live selection, which
   crosses elements happily — and then vanished on every later repaint. A mark
   still in the store, still counted by the badge, invisible. Reproduced at stack
   badge 4 with one highlight showing. `placeAcrossNodes` is the fallback both
   placers hand their misses to, and it wraps **one `<mark>` per text-node slice
   sharing the id**, not one around the range: `surroundContents` throws across an
   element boundary and `extractContents` would re-parent the `<strong>`'s
   contents to draw on them. `data-run` (`start`/`mid`/`end`) squares the interior
   edges, because `mark.hl`'s radius and 1px padding drew three abutting slices as
   three rounded pills — one phrase reading as three marks, a different and wrong
   statement about the document. `deleteHighlight` moved to `querySelectorAll`;
   it was the one consumer assuming one element per id.

6. **`prior` means "done with", not "read off disk".** `mark_sent` and
   `remove_from_stack` both set it now, `set_annotation` alone clears it, so the
   fade tracks the stack in both directions. The field's doc comment asserted the
   old contract in as many words and was rewritten, along with `set_annotation`'s
   and two test comments that repeated it. The pop handler called `refreshStack()`
   alone — panel redrawn, overlay untouched — so the fade arrived minutes later
   attached to whatever the reader did next; it and `saveAnnot` now repaint.
   `doc_from` still strips the flag on the way to disk, deliberately: `admit`
   makes everything prior on the way back in, so writing it would only let a
   hand-edited file assert a fade the reader's history does not support.

### Mistakes & deviations

- **Two wrong theories about the pane before the probe.** First that FitAddon
  double-counted the padding because `getComputedStyle` returns the content box —
  reasoned from the spec, and the arithmetic came out fitting, so the theory was
  discarded as *disproving* the bug. Then that the window was taller than the
  screen; `hyprctl` said 44 + 1027 ≤ 1080 and killed it. The lesson is the one the
  repo keeps relearning: this was a question about a real window and no amount of
  reading answered it. Three exploration subagents had also died on 529s, which is
  what pushed the work inline.

- **The first pane fix appeared to do nothing.** `ui/` is embedded into the binary
  by `tauri-build`, so an edit to `index.html` needs `cargo build` before it is
  live — the identical probe numbers across a relaunch were the tell. Cost one
  full debug cycle. Now a memory.

- **A harness check that could not fail.** The first version of the pane
  assertion compared `.xterm`'s height against `#pty-term`'s and was green both
  before and after the fix, because Chromium resolves that height per spec and the
  bug is WebKitGTK-only. Caught by reverting the CSS and watching it stay green —
  which is the whole reason each guard was mutation-tested rather than assumed.
  Replaced with a *structural* assertion: the padding is on `.xterm` and
  `#pty-term` has none. That one goes red.

- **A fixture that tested the wrong thing.** Verifying `placeAcrossNodes` in a
  real window, the first hand-written marks file gave the cross-markdown quotes
  nonzero `line_start`s, which the new guard correctly reads as "was anchored, now
  is not" — three "? still pertinent" chips and nothing painted. Not a
  regression: that is a mark whose text really did change, e.g. bold added around
  it in Neovim afterwards. `add_anchored` stores `(0, 0)` for the reachable case;
  with that, all four painted and the rail stayed empty.

- **An accidental screenshot of the wrong window.** `grim -g` against dreamd's
  coordinates captured the browser instead, because dreamd was on workspace 1
  while workspace 2 was active. Deleted, disclosed, and `hyprctl activeworkspace`
  is now checked first.

- One deviation from the plan, deliberately: the plan proposed leaving
  cross-node painting as a non-goal. The follow-up report made it the actual bug,
  so it was built.

### State

`cargo build`, `cargo fmt --check` and clippy `-D warnings` all clean.
**284** unit tests (was 278 at the start of the day, +6), `config_check` 57,
`mcp_check` 49, `marks_check` 75, `theme_check` 10 families, `locate_check` exit 0
over 611 fixtures, `node --test ui/paths.test.mjs` 10, `ui-check.mjs` **236** (was
222). `packaging/smoke.sh` in `paint` mode passes against the live Wayland session
— `SMOKE_XVFB=0`, because there is no Xvfb on this box.

**Every guard was proved to bite by mutation, not assumed.** Dropping the
`line_start` guard → 1 red; reinstating either `addStaleChip` on a placement
failure → 2 red; reinstating the pending chip → 2 red; reverting the menu
positioning → 2 red; moving the terminal padding back to `#pty-term` → 1 red;
dropping `prior` from `mark_sent` → 2 red, from `remove_from_stack` → 3 red;
disabling `placeAcrossNodes` → 5 red; wrapping the whole range instead of per
slice → **8** red, which is the check that the per-slice design is load-bearing
rather than merely sufficient.

Hand-verified in a real window on Hyprland: the composer's full bottom border
with air below it, and all four cross-markdown highlights painting as continuous
runs with bold weight and link colour intact.

**No perf number was obtained, and the attempt found out why.** `./perf/run.sh
quick` was run — the corpus was current, both Chromium scenarios and all three
criterion benches executed — and then produced nothing: **`jq` is not installed on
this box**, and `run.sh` collects every measurement through it (lines 179–181,
254), so all thirteen collection steps failed. Its `CRITERION_HOME` is inside a
`mktemp -d` that goes with the process, so the raw estimates are gone too. On top
of that the baseline is macOS-only and `run.sh` refuses a comparison off Darwin, so
even a successful collection would have had nothing to compare against. Installing
`jq` is the user's call; until then no tier is usable locally and an A/B of two
trees on the same machine is the only route to a regression signal here.

The one perf question this session actually raises is answered structurally
instead: `applyHighlights` now calls `scanTextNodes` a second time, but only
`if (crossNode.length)`, so a document whose marks all place inside one text node —
which is every corpus fixture — pays exactly nothing. The re-scan cannot be avoided
when it does fire: the wraps from the first pass have split the very text nodes the
first `doc` was built from.

Left open, and stated rather than glossed:

- **The send and pop fades end-to-end.** Both halves are covered — Rust unit
  tests, and a Chromium mock mirroring `remove_from_stack` — but a pop needs a
  click on the card's ✕ and there is no `wtype`/`ydotool` on this box, and a send
  needs a live Claude Code session.
- **Creating a cross-markdown highlight with the mouse.** Placement was driven
  from a marks file, which exercises the same `applyHighlights` path but not
  `triggerHighlight` → `add_highlight` → annotate.
- **A highlight over inline code reads warm-grey** rather than fully washed:
  `<code>`'s own background sits under a semi-transparent `<mark>`. The run is
  continuous and the mark is there. Left alone, because fixing it means styling
  `code mark.hl` and the standing line is that code's own colouring stays code's.

## 2026-07-29 — two bars fewer, on toggles

Linux stacked two rows of chrome above the reader: the GTK File/Edit/Help
menubar and, on top of it, the window's close/minimize/maximize bar. Both are
gone by default and both are now settings, under a new Settings → Window tab.

### What happened

1. **`config::Ui` gains `menubar` and `titlebar`**, both `bool`. `menubar`
   defaults to `false` everywhere; `titlebar` reads `TITLEBAR_DEFAULT`, which is
   `true` only on macOS. That is a per-platform *value*, deliberately not a
   `cfg` arm around the code that applies it, so `apply_chrome` stays one path
   on both platforms and a macOS user who hand-sets `titlebar = false` gets a
   frameless window rather than a key that silently does nothing. The asymmetry
   is real: `titleBarStyle: "Overlay"` puts the traffic lights *inside* the
   reading pane, so there is no second bar on macOS to reclaim.

2. **Both keys are denied to a repo-local `.dreamd.toml`.** They join
   `theme_css` and `agent.permission_mode` in `strip_untrusted`. The line drawn:
   `ui.tree_width` moves furniture *inside* the window and stays allowed, while
   `titlebar = false` takes the close button off a window a cloned repo has no
   business touching — and unlike a theme, moving to another repo does not undo
   it, because the reader has to find the settings panel to get the frame back.
   A test pins that the refusal is per-key rather than a dropped `[ui]` table.

3. **`main.rs` grew `apply_chrome` and `menubar_at_launch`**, next to
   `pin_native_theme`. The menubar is **attached and detached, never shown and
   hidden** — see the mistake below for why. `menubar_at_launch` decides whether
   the builder calls `.menu()` at all (`|| cfg!(target_os = "macos")`, because
   the macOS bar is app-wide and carries dreamd's only File → Open), and
   `set_menu`/`remove_menu` handle the live toggle. The builder chain had to be
   split across a `let` to make `.menu()` conditional. `apply_chrome` runs from
   `.setup()`, from `set_config` — which is what makes the toggles instant
   rather than restart-only — and from `adopt_root`, which re-reads config.

4. **Detaching the bar takes its accelerators with it**, because
   `remove_for_gtk_window` drops the window's accel group. Accepted rather than
   worked around: a menu that is not there should not own `Ctrl+Shift+O`. Nothing
   is stranded — the sidebar header's root field opens a folder with tab
   completion — and the row's sub-label and the README both say so.

5. **A fourth settings tab, Window**, rather than a section inside Themes: the
   Themes pane has live-preview-undone-on-close semantics, and this pane is the
   only route back once both bars are gone. `renderWindow` builds the rows from
   `WINDOW_TOGGLES` so the menubar row can be omitted on macOS, where
   `hide_menu` is a documented no-op and the toggle would be wired to nothing.

6. **Harnesses.** `config_check` gained a window-chrome write-back section (57
   checks). `ui-check.mjs`'s mocked `get_settings` now carries a `ui` table,
   which is what Rust actually sends, plus four checks that the tab lists both
   rows, starts from the payload, and writes `ui.titlebar` on click (226).

### Mistakes & deviations

- **The first implementation did not work, and only launching the app showed
  it.** `hide_menu()` from `.setup()` before `win.show()`, reasoning that
  `run_on_main_thread` resolves inline on the main thread so the hide would land
  first. It did land first — and the bar was still on screen. tao turns
  `Window::show` into `gtk_widget_show_all`, which recursively re-shows every
  child, and `show` is queued through tao's *window-request channel* while the
  hide runs inline, so `show_all` is always last and no ordering inside
  `.setup()` can win. Caught by running the debug binary under XWayland and
  reading `_MOTIF_WM_HINTS` plus a cropped screenshot: decorations were gone,
  File/Edit/Help was not. Rewritten to attach/detach, which leaves no widget for
  `show_all` to find.

- **Two doc comments then asserted the opposite of the truth** — that muda's
  `hide_for_gtk_window` keeps the accel group attached, which is correct about
  `hide_menu` and irrelevant once the code uses `remove_menu`. Both the
  `config::Ui` doc and the settings row's sub-label were corrected in the same
  pass, along with a pane note claiming "everything in the menubar has a
  keybind" — Open Folder does not.

- **The first verification ran on the wrong stack.** `GDK_BACKEND=x11` was used
  to make the window scriptable, but the machine is Hyprland: the bar the user
  actually sees is GTK's *client-side* decoration, not a WM frame. Noticed
  before drawing any conclusion about the titlebar from `_NET_FRAME_EXTENTS`
  being absent, which under XWayland means nothing. `gtk_window_set_decorated`
  governs both, so the mechanism was right regardless. The user hand-verified
  both toggles on the real desktop, which is where this stopped.

### State

`cargo build` clean; `cargo test --all-features` 278 passed; clippy clean with
`-D warnings`; `config_check` 57, `mcp_check` 49, `marks_check` 75,
`theme_check` 10 families, `node --test ui/paths.test.mjs` 10,
`ui-check.mjs` 226 — all green. The real binary was launched on both XWayland
and Wayland during the session; the user hand-verified both toggles round-trip.

**No perf tier was run, deliberately.** The baseline is macOS-only and `run.sh`
enforces it off Darwin — a `pass` here would write `perf/results/` with nothing
to compare against. The change adds one `set_decorations` call at startup, a
conditional `.menu()`, and panel JS that runs only when the panel opens; nothing
touches the render, locate or search paths. Worth a `perf-pass` on the Mac
before a release if the startup path is in question.

Left open: with the titlebar off, resizing an undecorated window is the
compositor's business and varies by WM — not addressed, and no report of it.

## 2026-07-29 — the pane earns its header

Seven asks about the agent pane, all from using it: the send icon was a
clipboard, the send took five seconds, the model could not be changed, and an
unregistered MCP server was indistinguishable from an agent that forgets to
resolve marks. Five changes built, two answered as already-there.

### What happened

1. **The clipboard icon means the clipboard.** `#btn-send` kept its markup but
   became `#btn-copy` → `copy_stack` (Ctrl+C); a new paper-plane `#btn-send`
   takes the rightmost, primary slot and runs Ctrl+Enter's verb. They were one
   button, which is why they now sit adjacent in that order.

2. **The five-second undo window is gone**, and so is the idle-quiet wait. D16
   traded wrongly: the regretted send is rare and Escape already interrupts a
   turn, while the delay was paid on every send and was most of what the loop
   felt like. `runStack` now calls `flowTick()` directly after `openPane`
   resolves, so on a warm pane the whole thing — queue, arm, release — happens
   inside the keypress and the 350ms interval never runs. D11's mid-turn queue
   went with it: Claude Code's composer queues a line typed during a turn, so
   dreamd guessing at idleness from outside was approximating something the TUI
   already does correctly.

   **`flow.rs` is unchanged.** The dedupe and the ask-order still hold, and
   `Phase::Undo` survives as a state a submission passes *through* — the
   frontend arming immediately is a frontend policy, not a fact about the state
   machine. Only the prose was corrected, in `flow.rs`, `main.rs`'s flow
   section, and `AppState::flow`'s doc.

   **One wait survives and it is not the old one.** A TUI still drawing its
   first frame drops the line entirely, so `noteBootQuiet` holds a submission
   until the child has been silent for 1.5s *once*, then latches ready forever —
   mid-turn included. `paintSendBar` lost its countdown and now names which of
   the three reasons it is waiting for (`pty.opening` exists only so a cold
   start reads "starting" rather than "not running", which is the same fact
   worded as a failure).

3. **Model chips in the pane header** — opus / sonnet / haiku. A live `/model`
   line typed into the running child, not a `--model` restart: the conversation
   in the scrollback is the thing the reader is mid-way through, and the
   permission-mode select pays that cost only because its flag genuinely cannot
   be changed any other way. `pty::Model` is the second closed enum in that
   file, so all three strings are compiled in and pinned by a test (tenet 3 —
   the far end may be a login shell, where `/model` is a path that does not
   exist). Aliases rather than dated ids, so a chip does not rot when a model is
   renamed. **No chip starts lit**: dreamd passes no `--model` and cannot honestly
   claim to know what is live; a restart clears it again.

4. **An MCP status strip at the top of the pane**, silent when healthy. New
   `mcp::server::Status` (serving + accepted-connection count), one per `spawn`
   so a retiring server on the previous root cannot write `serving = false` over
   its replacement — `adopt_root` swaps the whole `Arc`. Three distinct
   failures, because the fix for each differs: no repo, another dreamd owns the
   socket, or no agent has ever connected (that one names
   `claude mcp add dreamd -- dreamd mcp`). `clients` is deliberately **not**
   liveness — the shim connects per call — so the only honest question it
   answers is "has this ever worked", which is exactly the failure worth naming.
   Polled every 5s while the pane is open, and the poll stops itself once
   healthy.

5. **The stack and the document are granted by default.** Every launch now
   carries `--allowed-tools Read` plus dreamd's five MCP tools, in all four
   permission modes. Highlighting a passage and attaching a question to it is
   already the consent; a prompt for the stack lands in a terminal nobody is
   looking at, so the send appears to have gone nowhere. Nothing that writes is
   granted — an edit still asks, in whatever mode was chosen. Spelled out rather
   than wildcarded so a sixth MCP tool is a deliberate line. Implemented as a
   `macro_rules!` expanding to a literal, because `concat!` takes literals and a
   `const` would not do; the four commands are still four whole compile-time
   strings.

6. **Auto-submit and auto-accept-edits were already there** and were answered
   rather than built. `take_send` has always written the line then `\r`
   separately; `accept edits` has been dreamd's default (not Claude Code's)
   since t5.

### Mistakes & deviations

- The first grant test asserted `!cmd.contains("Edit")` and failed on
  `--permission-mode acceptEdits`. The test was right to fire — the search was
  answering the wrong question. Rewritten to split at ` --allowed-tools ` and
  compare whole words, plus an exact count of six.
- `refreshMcpStatus` fed a null straight into `paintMcpStatus`, which
  `ui-check.mjs` caught as three pageerrors before anything else did. A missing
  reply now leaves the strip alone rather than accusing a healthy socket.
- The first rewrite of the T6 harness block used "the child spoke 0ms ago" for
  the cold-start state, which ripens into "quiet for 1.5s" somewhere between two
  assertions and would have been flaky. Replaced with `coldPane()` —
  `lastData = 0`, the state `noteBootQuiet` will not leave on its own.
- perf/quick flagged `chromium.scroll…composite_ms` twice (+28%, then +50%),
  which looked plausible given the session declared new CSS. A/B'd directly
  against a HEAD worktree, 3 reps per arm: this tree 4.33–5.52ms, HEAD
  5.08–6.40ms. **HEAD was the slower arm** — the row is machine noise against a
  baseline this box no longer matches. The Rust rows did not reproduce at all.

### State

`cargo build`, fmt and clippy clean. 275 unit tests, `mcp_check` 49 (two new
`Status` assertions), `config_check` 49, `theme_check`, `marks_check` 75,
`node --test ui/paths.test.mjs` 10, `ui-check.mjs` 222 — all green.

Both new guards were proved to bite rather than assumed to: adding `Edit` to the
grant list fails the Rust test, and reinstating the idle wait in `paneReady`
fails "a send during a running turn still goes" first.

`perf-pass` skipped at the user's instruction; the quick-tier A/B above is the
only performance evidence this session has, and it says nothing moved.

**The GUI itself is unverified by machine, as always.** `ui-check.mjs` asserts
what the page knows, not what it paints — the chips, the status strip and the
one-press send want a hand check in a real window.

## 2026-07-28 — t5-agent-pane

- `agent.permission_mode` reaches the child: four compiled-in commands, a `match`
  over the closed enum, no format string (tenet 3). `pty_spawn` reads the mode
  from `AppState` rather than off the wire.
- New `agent_prefs` command — position + mode, fetched on the pane's first open,
  not at boot.
- `agent.position` implemented. `#main-wrap` stays a flex column; the right dock
  is a two-track grid gated on a `body.pane-open` class, because a `display:none`
  grid item still holds its track open.
- Restyle per D18/D19: liveness dot, header, terminal padding, themed xterm
  scrollbar, `cursorAccent`, inactive-selection colour. Grid stays monospace and
  the 16 ANSI colours are left alone — a reading-coloured red would make a
  removed diff line look kept.
- Escape closes the pane from inside the terminal (D12). Costs Claude Code's
  interrupt; the comment says so. Double-Escape deliberately not built.
- Permission-mode select in the header: staged, warns that a restart loses the
  conversation, then writes `set_config` *before* restarting.
- Verified: `cargo test --all-features` (231), `config_check` (49), `ui-check.mjs`
  (130), fmt, clippy `-D warnings`, `cargo build`. No perf tier run.
- Teeth proven by mutation: wrong `match` arm, `$` in a command literal, Escape
  claim removed, `agent-right` toggle disabled, restart-before-write, silent mode
  apply, refit removed. The last one is *not* caught — the `ResizeObserver`
  covers every path the harness can drive; the explicit `fitPane` is belt only.
- Hand check: everything visual. ui-check asserts what the page knows, not what
  it paints, and the GUI cannot be driven here.

## 2026-07-28 — t4-chrome

- Tree drag: handle on `#sidebar`'s right border, width in `--tree-width` on
  `<html>`, persisted to `ui.tree_width` debounced 400ms. Past 140 it collapses
  (D20); 600 is the cap, both mirroring `config::TREE_WIDTH_*`.
- Outline floats top-right over the reader, fades by CSS, closes on a heading
  click and on any reader scroll (D21) — a scroll rather than a timeout, which
  cannot strand a half-faded panel.
- `#repo-name` is now an `<input>`: basename unfocused, full path focused, `~`
  expansion, Tab completion, error state that leaves the current root (D22).
- New Rust: `rootfield` (absolute-only, existing directory only, directory
  names never paths, dot-dirs only when asked, 200 cap) behind two commands,
  `complete_directories` and `set_root`; plus `get_ui` for the boot width.
- Drag strip: 10px `data-tauri-drag-region` at `z-index: 0`, titlebar at 1, not
  in the view-mode hide list — the window is never un-draggable.
- Verified: `cargo test --all-features` 244, `config_check` 49, `ui-check.mjs`
  147, `node --test ui/paths.test.mjs` 10, fmt/clippy/build clean.
- Teeth proven by mutation: 5 in `rootfield` (absolute check, dir-only filter,
  hidden filter, listable-target check, cap), 6 in `ui/` (boot width, max
  clamp, collapse-past-min, scroll-close, float position, error flag).
- **Hand check, still:** all four are visual, and the drag strip cannot be
  asserted at all — the only proof is a view-mode window moving when you drag
  its top edge.

## 2026-07-28 — T2: the config surface the agent pass needs

A short, deliberately mechanical thread: the second of the seven in
`docs/plans/agent-ui-implementation.md`. It adds no behaviour — it adds the keys
the behaviour will read, so T4 and T5 can be about the pane and the chrome rather
than about plumbing. Done in a worktree and landed through a PR rather than
straight to main, at the user's request.

### What happened

1. **`[agent]`, with a closed enum on each key.** `position` is `bottom`
   (default) or `right`; `permission_mode` is `default` / `accept-edits`
   (default) / `plan` / `bypass-permissions`. Enums rather than strings, because
   T5 turns `permission_mode` into a launch flag and tenet 3 wants that to be a
   match over four literal `const`s, never a format string. A typo is rejected
   rather than clamped — there is no nearest sensible permission mode, and the
   failure this enum exists to prevent is an agent silently running in the wrong
   one.

2. **`ui.tree_width`, clamped rather than validated.** 140–600, defaulting to
   260, which is the sidebar's existing fixed width. The clamp is a
   `deserialize_with`, so every reader sees a usable number and T4's drag handle
   can persist without round-tripping through a validator; the file keeps
   whatever was written. Out of range costs the nearest usable tree. A *negative*
   width is still rejected outright — it never reaches `u32` — which is the same
   shape `mode = "sepia"` already had.

3. **The denylist became a function.** `.dreamd.toml` is repo content, and
   `agent.permission_mode` is the second key it may not set: a repo you have not
   read yet does not get to choose what your agent may do unasked. Rather than
   adding a second inline `remove` to `Config::load`, both refusals moved into a
   pure `strip_untrusted(&mut Table) -> Vec<(key, why)>`. That is the `guard`
   argument applied one level down: a tenet enforced where a unit test can reach
   it, instead of only where `config_check`'s sandboxed `XDG_CONFIG_HOME` can.
   The warning line also got its path back in front of its explanation.

4. **`keymap.send_stack_tmux`, the hidden binding of D6.** `Option<String>`,
   `None` by default, `skip_serializing_if` — so "Reset all shortcuts", which
   patches the global file with `default_keymap()`, cannot clear a value set by
   hand. The settings panel never offers it because `KEY_ACTIONS` in `app.js` is
   a hand-written list. Nothing dispatches it yet: the frontend half belongs to
   T6, where Ctrl+Enter stops being the tmux path and `send.rs` would otherwise
   become dead code.

5. **Proved the new guards have teeth.** Three mutations, each run against both
   the unit tests and `config_check`: defanging the `permission_mode` strip
   (`remove` → `get`) turned a repo-local `bypass-permissions` into the effective
   mode and went red in both; deleting the `deserialize_with` attribute went red
   on three clamp assertions; giving `send_stack_tmux` a default binding went red
   on the two that pin it absent. Restored, and green again.

### What this leaves

`cargo test --all-features` at 219, `config_check` at 49, clippy clean, and
`theme_check` / `mcp_check` / `marks_check` / `paths.test.mjs` unmoved. T5 and T6
are unblocked. No perf tier was run: nothing here is on the render path, and the
whole change is fields on a struct read once at load.
## 2026-07-28 — T1: the store learns what "sent" and "last session" mean

First thread of `docs/plans/agent-ui-implementation.md`, built in a worktree
(`worktree-t1-store-semantics`) and opened as a PR rather than pushed to main,
because the rest of the plan's threads read this thread's output and a reviewable
diff is worth more than a fast landing here.

T1 is deliberately the whole plan's foundation: every visual decision downstream —
T3's fade, T4's rail, T6's pending glyph — reads a field this thread adds. It is
also entirely pure, so it is all testable.

### What happened

1. **`Highlight` gained two fields.** `sent_at: Option<u64>` is the pending stamp,
   persisted, and lives beside `state` rather than inside it because a passage can
   go stale *while* it is out with an agent and a single enum would have to pick
   one of the two and would pick wrong (D5). `prior: bool` is "made in an earlier
   session", and its entire implementation is that `marks_file::admit` sets it on
   everything it admits — no clock, no session id, no schema field (D4).

2. **`Store::mark_sent`** stamps the ids it is given and removes *exactly* those
   from the stack. Not the whole stack: `Send selected` sends a subset, and
   clearing everything would silently discard questions that were never asked
   (D17).

3. **`set_annotation` grew two rules.** It clears `prior`, because the fade means
   "untouched this sitting" and annotating is touching (D13). It does *not* clear
   `sent_at`: re-annotating a pending mark stacks a second question about the same
   passage rather than retracting the one already asked (D14). The second rule
   needed no code — once `mark_sent` has taken the id off the stack, the existing
   re-push does exactly this — but it needed a test, because "no code" is
   indistinguishable from "not implemented" until something pins it.

4. **`resolve` clears `sent_at`.** One line, and it is what makes the pending
   glyph go away.

5. **`send_stack` calls `mark_sent`** with the ids it actually sent — from the
   assembled pairs, not from the argument, since `selected_pairs` skips an id
   naming nothing and an unannotated mark. Only after a successful send: a failure
   leaves the stack as it was, which is the state a retry starts from. The
   clipboard fallback counts as a send, on the grounds that a stack that looks
   unsent while the text sits on the clipboard is the one that gets asked twice.

6. **Eleven tests, each proven to have teeth.** Every guard was broken, watched go
   red, and restored — seven mutations on the first pass and two more after the
   serde change below. One of them found a real hole; see the deviations.

### Mistakes & deviations

- **The plan specified `#[serde(skip)]` on `prior`, and that would have broken
  T3.** One derive serves two boundaries: the marks file *and* the IPC reply
  `get_highlights` returns. A plain `skip` keeps the flag off disk and out of the
  frontend's hands in the same stroke — and T3's job is `app.js` setting
  `data-prior` from that field. It is now
  `skip_deserializing` (a hand-edited or copied file can never assert that a mark
  is prior; only having been admitted decides that) plus
  `skip_serializing_if = "is_false"` (false stays off the wire, so the frontend
  must read an absent key as false), and `marks_file::doc_from` clears the flag on
  the copy it writes. Both halves of the plan's intent hold, and
  `prior_still_reaches_the_frontend` is what fails if someone simplifies the
  attributes back.

- **The first version of `prior_never_reaches_the_file` was toothless, and the
  teeth check is the only reason that is known.** It annotated the prior mark
  before serialising — which clears `prior` (D13), so no mark in the store carried
  the flag and the assertion held whatever `doc_from` did. Deleting the strip left
  the test green. The mark is no longer annotated, the premise assertion now says
  *why* in the failure message, and the mutation goes red.

### State

`cargo build` clean, 220 tests pass (11 new), `cargo clippy --all-targets
--all-features` clean, and `marks_check`, `config_check`, `theme_check` and
`mcp_check` all pass. No frontend change, so `ui-check.mjs` is unaffected. No perf
run: the plan gates none on this work and nothing here is on the render path — one
`/perf-quick` at the end of the pass, after T6.

T2 (config surface) is unblocked and independent; T3, T4's rail work and T6 now
have the fields they read.

## 2026-07-28 — CI that runs the program

The session opened with a question: if a commit passes CI, will it work when
pulled onto the Arch box? The answer was no, and the interesting part was *why* —
nothing in the repo had ever started dreamd. It ends with CI that launches the
program on Linux and installs its artifacts on four distros that did not build
them.

### What happened

1. **Landed the two things sitting in the working tree.** `src-tauri/src/webkit.rs`
   sets `WEBKIT_DISABLE_DMABUF_RENDERER` at the top of `main`, before any thread
   exists: WebKitGTK's DMA-BUF renderer allocates through GBM, which fails on the
   NVIDIA proprietary driver, and on Wayland the malformed `wl_buffer` is a
   *protocol* error, so the compositor drops the connection and GDK aborts inside
   GTK init with nothing to catch. Detection is a probe of
   `/proc/driver/nvidia/version` rather than a `#[cfg]`, so the module compiles
   and runs identically everywhere and answers "no" on macOS and on Mesa. An
   existing value always wins, including `=0`. `.github/workflows/canary.yml`
   landed alongside it.

2. **Audited what green CI actually guaranteed, and it was less than it looked.**
   Nothing launched the program on any runner; `ui-check.mjs` drives Chromium
   behind a stub of the Rust commands while the machine runs WebKitGTK 2.52;
   `package-smoke` only asserted the artifacts were non-empty files; and every
   Linux runner is `ubuntu-22.04`, the exact inverse of a rolling development box.
   A commit that compiled, tested green and aborted inside GTK init would have
   been green everywhere.

3. **Wrote a plan (P0–P5) and implemented the first two tiers.** The organising
   insight was that `ui/app.js:149` emits a `first_paint` perf mark that is
   unreachable unless GTK initialised, wry built a real WebKitGTK webview,
   `frontendDist` loaded, the CSP admitted both classic scripts and IPC completed
   in both directions — the whole startup path in one grep, and the instrumentation
   already existed.

4. **`packaging/smoke.sh`.** Launches dreamd under Xvfb in its own fixture repo
   with `XDG_CONFIG_HOME` pointed at a scratch directory, so tenet 2 stays true of
   a CI run. `SMOKE_EXPECT=paint` waits for `first_paint` and needs `--features
   perf`; `SMOKE_EXPECT=window` is for a release artifact, which carries no
   instrumentation and must be smoked as shipped — the MCP socket appearing (bound
   in `.setup`, hence downstream of a window), a `WebKit*` descendant process, and
   survival. It proves a window, not a page, and says so. Needs nothing but bash,
   coreutils and `/proc`, which is what lets it run in a bare container beside a
   downloaded artifact. Verified locally in both modes against the real binary and,
   as a negative control, against `/bin/sh -c 'sleep 300'`, which it correctly
   refuses to call "up".

5. **Wired it into three workflows.** `ci.yml` gained a `launch` job with two arms,
   with and without the DMA-BUF renderer; the accelerated arm is
   `continue-on-error` because a hosted runner has no GPU. `canary.yml` launches
   against a rolling webkit and then against the AppImage it just bundled.
   `perf.yml` gained `install-smoke`: the Linux bundles built on `ubuntu-22.04`,
   uploaded with `smoke.sh` riding along (the containers have no checkout), then
   installed and launched in `ubuntu:24.04`, `debian:12`, `archlinux:latest` and
   `fedora:latest`. The deb goes through `apt-get install ./`, so the bundler's
   `Depends` is checked by being resolved. The glibc 2.35 floor was an argument;
   it is now an assertion.

6. **Three rounds of real failures, each diagnosed from evidence.** See below.

7. **Dropped `libxdo-dev` from `perf.yml`**, which `ci.yml` had removed on
   2026-07-27 while this file was missed — so CLAUDE.md's claim that it was gone
   from both runners was true of one of them. `libxdo` appears nowhere in
   `Cargo.lock`, and the new `launch` job starts the GUI on a runner that never
   installs it, so the runtime side is covered too now.

### Mistakes & deviations

- **`set -euxo pipefail` in a container.** My own bug, and it failed in the most
  misleading way possible: inside a container the runner resolves the default
  shell to `sh`, which Arch and Fedora symlink to bash and Ubuntu and Debian point
  at dash. Two arms passed and two died on `illegal option -o pipefail`, from a
  `set` line that had nothing to do with the artifact under test. `shell: bash`
  now makes the four arms differ only in the distro, which is the point of four.

- **Assumed the canary's `failed to run linuxdeploy` was the `SHT_RELR` failure it
  was written to watch for. It was not.** Running the same command on the
  development box produced all three artifacts, which ruled out distro drift and
  left "the container lacks something a real Arch install has". Guessed
  `squashfs-tools`, then killed that theory by noticing it is absent locally while
  the local bundle succeeds. Stopped guessing at that point and made the failure
  legible instead: `build.sh` gained `VERBOSE=1`, which appends `--verbose` so
  tauri-bundler stops discarding linuxdeploy's stderr, and `canary.yml` sets it
  unconditionally. It named the real problem on first use —
  `cp: cannot stat '/usr/lib/gdk-pixbuf-2.0/2.10.0'`. No package declares that
  directory: gdk-pixbuf2 compiles the common loaders in and ships none, modern
  librsvg no longer installs one, `loaders.cache` is generated by a pacman hook and
  owned by nothing, and on this machine the path exists only because `libwmf`
  happens to be installed. A container built from the README's dependency line has
  no `/usr/lib/gdk-pixbuf-2.0` at all. The lesson is recorded in CLAUDE.md as the
  *third* reading of a red canary, alongside "Arch moved" and "this commit broke
  it".

- **`debian:12` then died on `xauth command not found`** — `xauth` is a
  *Recommends* of `xvfb`, so `--no-install-recommends` drops it, while
  `ubuntu:24.04` happened to have it and passed. Exactly the kind of difference
  four arms exist to surface.

- **Burned the unauthenticated GitHub API budget** with a watcher polling every
  30s across three runs, which cost 60 requests in six minutes and left the next
  hour blind. `gh` is installed and authenticated now; it read the canary's log
  directly and ended the guessing in one command.

### State

`cargo build` clean, 208 tests pass. On `e0fee5c` every workflow was green: `ci`
(both `launch` arms, `rust` on both platforms, `ui`), `perf` (all four
`install-smoke` arms, both `package-smoke` arms, both `bench` arms) and `canary`,
whose step list confirms it launched both the instrumented build and the AppImage
it had just bundled. `ci` is green on `c25dec0`; the `perf` run for that commit —
the one that proves the `libxdo-dev` removal against a full Linux bundle — was
still in flight at wrap-up.

**`perf-pass` was deliberately not run.** The only `src-tauri/` change this session
is one `Path::exists()` at process start, and `perf/baseline.json` is macOS-only —
`run.sh` refuses a comparison off Darwin, so five minutes here would have produced
numbers with nothing to compare them against. Nothing about the change is on a
measured path beyond a single stat during cold start.

Open, and named in the files rather than left implicit: the AppImage's
self-containment is still unchecked, because every `install-smoke` arm has webkit
installed by the time it runs; the NVIDIA/Wayland abort itself remains a hand-check,
since no hosted runner has the driver or a compositor; and P1–P5 of the plan
(desktop-integration invariants, a `tauri-driver` job against real WebKitGTK, a
headless-Wayland arm, CLI-surface coverage, `dreamd --doctor`) are outstanding.

## 2026-07-27 — the Linux pipeline, actually run

The previous session wrote the Linux half and shipped it untested — CI was to be
its first execution. This one ran it on a real Arch machine instead. Everything
CI checks is green on Linux; the *packaging* pipeline was not, and four defects
came out of running it that no amount of reading had found.

### What happened

1. **The environment first.** Arch, glibc 2.44, Wayland, rustc 1.93.0. The
   README's Arch dependency line (`webkit2gtk-4.1 gtk3 librsvg openssl patchelf`)
   turned out to be exactly right — it was tested as written rather than as a
   superset, which is what surfaced the phantom in item 4. `dpkg` had to be added
   for `dpkg-deb`; it is an artifact-only dependency and the README now says so.

2. **Every CI gate passes on Linux.** fmt, clippy `-D warnings`, 205 tests
   (including the real-pty ones), `config_check` 34, `theme_check` 10 families,
   `mcp_check` 46, `marks_check` 75, `locate_check` over 611 fixtures,
   `paths.test.mjs` 10, `ui-check.mjs` 108. Beyond the harnesses, checked by hand
   what only a real kernel shows: the socket binds `0600` inside a `0700`
   directory, and after `SIGTERM` the crash leftover is unlinked and rebound
   rather than demoting the next launch to secondary. `dreamd mcp` answers
   `initialize`/`tools/list` from the compiled-in schema with no GUI running.

3. **The blocker: the AppImage cannot be built on a modern distro.**
   `packaging/build.sh` died with a bare `failed to run linuxdeploy` — Tauri
   discards the tool's stderr at default log level (`linuxdeploy.rs:207-213`), so
   that string is the entire diagnostic. Running linuxdeploy by hand showed its
   own bundled binutils `strip` failing on every system library with
   `unknown type [0x13] section '.relr.dyn'`: it predates `SHT_RELR`, which
   distributions now emit for packed relative relocations. `NO_STRIP=1` clears it
   completely and the full pipeline exits 0.

   `ubuntu-22.04` predates RELR, which is why CI never saw this and why
   `release.yml` must *not* set the variable. Documented at three depths:
   `packaging/build.sh`'s Linux `case` arm (where you land when it fails), a
   README troubleshooting section beside the inotify one, and CLAUDE.md's
   Packaging section — which had claimed local reproducibility unqualified.

4. **`libxdo` is a phantom.** `libxdo-dev` was installed on both runners and
   listed in the README's Debian line, but `libxdo` appears nowhere in
   `Cargo.lock` and the build succeeds without it. The Arch and Fedora lines had
   always omitted it and were the correct ones. Dropped from `ci.yml`,
   `release.yml` and the README, with a comment in `ci.yml` recording that
   removing it changed nothing so it does not come back on a hunch.

5. **The desktop entry could not open markdown.** The bundler's default template
   emits neither a `MimeType` nor a field code, so dreamd never appeared under
   "Open With" for a `.md` file on any desktop — for a markdown reader, the whole
   integration. Added `packaging/dreamd.desktop` and pointed
   `bundle.linux.deb.desktopTemplate` at it. One template covers all three
   artifacts: the AppImage's AppDir comes from `debian::generate_data` and the
   `.tar.gz` is that same staged tree.

   `%f` and not `%F`, deliberately: `Cli::path` is a single `Option<PathBuf>`, so
   a multi-select would hand clap several paths and it would refuse them. The
   rationale is a handlebars `{{!-- --}}` comment rather than `#` lines, because
   `#` comments are valid in a Desktop Entry and would therefore ship into every
   user's `/usr/share/applications`.

   Checked the case this newly enables rather than assuming it: a file-manager
   launch means cwd `/`, and `resolve_target` (`lib.rs:73-93`) roots a file
   argument at the *cwd's* repo with `has_repo=false`, so dreamd opens the
   document beside an empty sidebar instead of walking the filesystem. Confirmed
   by launching from `/`.

6. **The icon set was missing the size Linux uses most.** `bundle.icon` fed
   hicolor `32`, `128` and `256@2` only. `cargo tauri icon` emits no 48², so
   `48x48.png` is now rendered from the same `favicon.svg` with `rsvg-convert`
   (validated against tauri's own renderer: absolute error 1.2 over 4096 pixels
   at 64²). `64x64.png` already existed in the repo and had simply never been
   listed. Both were added *after* the first array entry, so the baked window
   icon is still `128x128.png` — confirmed by the single 65,536-byte
   (`128 × 128 × 4`) blob in the codegen out directory.

7. **Toolchain.** This machine's stable was 1.93.0 while `ci.yml` pins 1.97.1 —
   the pin is meant to track the development machine, and there are now two.
   `rustup update` brought it to 1.97.1 exactly, so no workflow bump was needed;
   the whole suite was re-run on it.

### Mistakes & deviations

- Read `tauri-bundler`'s `debian.rs` and concluded the `.deb` would ship with no
  `Depends:` at all — a real shipping bug if true. It was not: `tauri-cli`
  pre-populates them (`rust.rs:1443-1444`) and the built package carries both,
  duplicated. Corrected by building the thing and reading `dpkg-deb -I` instead
  of trusting a source read one crate too far upstream.
- Invoked `build.sh` as `build.sh … | tail`, so the pipeline reported the exit
  code of `tail` and a *failed* build looked successful. The failure was caught
  by reading the log, not the status.
- After the first AppImage failure, a rerun died in the GTK plugin instead and
  was briefly treated as a second, separate defect. It was the first run's
  half-populated AppDir: the plugin `ln -s`es into it and aborts on a link that
  already exists. A clean build from a wiped `bundle/` succeeded, and the rerun
  trap is now documented alongside the fix.

### State

- `cargo build` passes; the full CI suite is green on Linux at rustc 1.97.1.
- `NO_SIGN=1 NO_STRIP=1 packaging/build.sh x86_64-unknown-linux-gnu` exits 0 and
  produces all three artifacts plus checksums. `desktop-file-validate` accepts
  the entry as shipped in the `.deb`, the `.tar.gz` and the AppImage; the icon
  set is 32/48/64/128/256@2 in all three; both channels launch and stay up.
- `perf-pass` deliberately not run. No Rust changed — the diff is one config
  file, one PNG, docs and CI — and `perf/run.sh` refuses a baseline comparison
  off Darwin, so it would have produced uncomparable numbers rather than a
  regression signal.
- **The macOS arm of this diff is unverified here.** `bundle.icon`'s additions
  and the new `linux` block are additive and leave the first PNG untouched, but
  only CI's `macos-14` job will actually prove it.
- Left alone, both known and both upstream: the `.deb`'s duplicated `Depends`,
  and the absent `usr/share/doc/dreamd/copyright` despite `licenseFile` being set.

## 2026-07-27 — Linux as a shipping target

Asked for three things at once: cloud sessions that can actually run CI/CD, full
testthroughs on Linux, and an Arch dev machine that is not a second-class
citizen. All three turned out to be downstream of a single five-error build
failure. They landed; the Linux half is written but has never run — CI is its
first execution.

### What happened

1. **The one blocker.** `main.rs:1292-1295` referenced `dreamd::menu::build`,
   `menu::OPEN_FOLDER/OPEN_FILE` and `open_target`, all
   `#[cfg(target_os = "macos")]`. The library had always compiled on Linux; the
   `[[bin]]` had not, since `docs/session-log.md:1189` recorded it. Ungated
   `menu` (`lib.rs:20`), `open_target` and `adopt_root` (`main.rs`), and moved
   `rfd` out of `[target.'cfg(target_os = "macos")'.dependencies]`. The call site
   needed no edit at all — that was the test that the fix was the right shape.

   Ungating `adopt_root` was the point of the exercise, not a side effect: it
   carries the config reload, the re-walk, the watcher re-arm, the marks flush
   and the socket retirement, and gating it was what kept all of that from ever
   being *compiled* off macOS.

2. **The menu could not simply be shared, and finding out why changed the
   plan.** The plan said "ungate `menu.rs`, muda exposes every
   `PredefinedMenuItem` on every platform". True of the API, false of the
   backend: `is_item_supported!` in muda 0.19.3's `platform_impl/gtk` admits only
   `Separator`, `Cut`, `Copy`, `Paste`, `SelectAll`, `About` and custom items.
   `Undo`, `Redo`, `Minimize`, `Maximize`, `Fullscreen`, `Hide`, `HideOthers`,
   `ShowAll`, `Services`, `CloseWindow` and `Quit` are *silently dropped*, so the
   macOS bar would have rendered a "dreamd" submenu and a Window submenu with
   nothing in either.

   Worse, and caught in the same read: Cut/Copy/Paste/SelectAll only `set_accel`
   on their label (cosmetic — `Ctrl+C` still reaches the webview), but **custom
   items register a real accelerator on the window's accel group**. So
   `CmdOrCtrl+O` on Linux would be consumed before the webview saw it and
   `keymap.toggle_stack` (`Ctrl+O`, `config.rs:162`) would silently stop working
   — not a double-fire, a disappearance. `menu::build` now has two arms: macOS
   unchanged, Linux gets File/Edit/Help with `Ctrl+Shift+O` / `Ctrl+Alt+O`.

3. **Dropped the per-platform window config, deliberately.** The plan (following
   `docs/todo2.md:51-60`) wanted `titleBarStyle`/`hiddenTitle` moved into a
   `tauri.macos.conf.json`. Tauri merges platform overlays with `json_patch`
   (RFC 7386), which **replaces arrays wholesale** — so the overlay would have
   had to duplicate the entire `app.windows[0]` object and then drift from the
   base. `titleBarStyle` is already inert on Linux, so the split bought nothing.
   Only `tauri.linux.conf.json` was added, carrying `bundle.targets` and the deb
   dependencies.

4. **Runtime papercuts.** `pty::login_shell`'s `/bin/zsh` fallback became a
   `DEFAULT_SHELL` const — zsh on macOS, `/bin/sh` elsewhere, since zsh is not
   guaranteed to exist on Linux. `arboard` is `default-features = false`, which
   drops `wayland-data-control`; re-enabled for `cfg(target_os = "linux")` only,
   or the clipboard fallback in `send.rs` would fail on every Wayland session —
   the Arch default. Documented the inotify asymmetry in `watcher.rs`: one
   FSEvents stream on macOS is one watch *per directory* on Linux against a
   machine-wide budget.

5. **Packaging, and a better tarball than planned.** `build.sh`'s first `case`
   already yielded `appimage,deb` for `*-unknown-linux-gnu`; only the packaging
   arm was `exit 1`. The plan said tar the bare release binary. Instead it tars
   the bundler's **staged deb tree** (`$BUNDLE/deb/*/data`), which the bundler
   leaves in place and which already holds the generated `.desktop` and the
   hicolor icons — so the `.tar.gz`, the `.deb` and the AppImage carry
   byte-identical desktop integration and `install.sh` and the PKGBUILD both
   just copy it, instead of two scripts inventing a second copy that drifts.
   `shasum` gained a `sha256sum` fallback (macOS ships one, Linux the other).

   `install.sh` grew a Linux branch: `~/.local/bin` for the binary,
   `~/.local/share` for the desktop entry and icons, never root, `ditto` and
   `codesign` confined to the Darwin arm. New `packaging/PKGBUILD.tmpl` — a
   `-bin` package in the same shape as `cask.rb.tmpl`, forced by the fact that
   `tauri-bundler 2.9.4` has **no pacman backend** (verified against the crate
   source: its `bundle/linux/` is appimage, debian, rpm).

6. **CI parity.** `ci.yml`'s `rust` job is now a `macos-14` / `ubuntu-22.04`
   matrix running identical steps — fmt, clippy `-D warnings`, test, build, then
   `config_check`, `theme_check`, `mcp_check`, `marks_check`, `locate_check`.
   The four `config_dir()` harnesses gain the most: the socket, the 0600/0700
   modes and the atomic rename are the same code on a different kernel, and none
   of it had ever run anywhere but macOS. `release.yml`'s commented Linux row is
   live, plus an `aur` job behind `vars.PUBLISH_AUR` that renders and *uploads*
   the PKGBUILD without pushing — pushing needs an AUR deploy key, which is its
   own decision.

7. **Perf on both platforms.** `stat -f%z` → BSD-then-GNU fallback. The `time`
   fix mattered more than the flag suggests: macOS `/usr/bin/time -l` reports
   peak RSS in **bytes**, GNU `time -v` in **kilobytes**, so parsing one with the
   other's assumption is a silent 1024x error, plausible enough to sit in a
   results file unnoticed. `startup.sh` now branches and normalises to bytes, and
   reports `null` rather than a fabricated `0` when GNU time is absent.
   `run.sh` refuses to diff against `perf/baseline.json` off Darwin, and refuses
   `--update-baseline` outright — the baseline is one arm64 Mac's numbers, and a
   Linux diff against it is noise wearing a regression's clothes.

   New `.github/workflows/perf.yml`, per the user's ask for a not-too-deep CI
   perf framework covering both pipelines: a `bench` job running the quick tier
   on both (informational, gating nothing, with an opt-in same-runner A/B via a
   `compare_ref` dispatch input) and a `package-smoke` job running
   `NO_SIGN=1 packaging/build.sh` on both. The second is the one that earns its
   keep — a tagged release is frozen, so a Linux arm found broken at tag time
   cannot be re-run.

### Mistakes & deviations

- **Two plan items were wrong and were corrected mid-implementation**, both
  found by reading the dependency source rather than trusting the API surface:
  the shared menu (item 2) and the per-platform window config (item 3). Neither
  had shown up in planning because both look correct from the outside.
- The plan's `Ctrl+O` accelerator for Linux would have silently broken
  `toggle_stack`. Caught by checking the default keymap against what muda's GTK
  backend actually registers, not by testing — there is no Linux machine here to
  test on.
- The plan proposed a `src-tauri/src/platform/` module per `docs/todo2.md`. It
  was not needed: the whole cfg surface came to three items (`menu::build`'s
  arms, `trash_context`, `pty::DEFAULT_SHELL`), each already living in the module
  it belongs to. Noted in `todo2.md` that `platform/` should wait until the
  Mac-only window chrome gives it something to hold.
- First draft of `perf.yml`'s step summary dumped whole nested JSON blobs for the
  Chromium section; rewritten to walk to numeric leaves and keep `p50`/
  `total_ms`/`wall_ms`.

### State

`cargo build`, `cargo fmt --check`, `cargo clippy --all-targets --all-features
-D warnings` and 205 tests all green on macOS. `config_check` 34, `theme_check`
20, `mcp_check` 46, `marks_check` 75, `locate_check` 0 disagreements,
`paths.test.mjs`, `ui-check.mjs` 108. `NO_SIGN=1 packaging/build.sh
aarch64-apple-darwin` produces a correct `dist/`, and `perf.yml`'s artifact
assertion was dry-run against it.

Both new guards were proven to bite rather than assumed to: re-gating
`open_target` reproduces the same `E0425`s, and inverting the Darwin test makes
`--update-baseline` exit 2 without touching `perf/baseline.json`. The Linux menu
arm was type-checked by temporarily flipping its `cfg` and building it.

`perf-pass`: 11 rows red against the committed baseline, but that baseline is a
`deep` run and drifts. A/B'd against the previous `pass` run on the same machine
it is 5 regressed / 23 improved with the entire Rust bench section unmoved, and
the alarming baseline rows (`d:ipc_get_highlights` +7385%, `spread_ms` +1591%)
are absent from the A/B. The 5 remaining are integer- or sub-millisecond-
quantized and have no mechanism in this diff — on macOS these changes compile to
essentially the same code. The loop improved: `save_to_paint_ms` 3055 → 2246ms
(−26.5%), `events_per_save` 0.875. The machine was not quiet (load 2.31).
`perf/results/pass-055dbbf-20260727-223410.json`.

**Nothing here has run on Linux.** `cargo build`, the five harnesses, the
AppImage/deb/tarball and the GTK menu shape are all first-run-in-CI.

*Follow-up, same session:* the README's "No session state is persisted" and "No
persistence by design" were both left over from before tenet 2's step-4
amendment and were fixed on request. The intro now states what actually
persists, and a new **Marks on disk** section documents the file, the lazy
per-file re-anchor, the primary/secondary claim and the `marks prune` flags —
each verified against the code or `--help` rather than written from memory,
which caught two wrong flag descriptions and an over-absolute claim about
code-block highlights (single-token ones do paint).

## 2026-07-27 — Step 5: the embedded Claude Code pane, and a spike that opened the gate

Step 5 of `docs/plans/agentic-mcp-persistence.md`, the last one and the one
designed to be cut. It was gated on a signing question — dreamd ships
`hardenedRuntime: true` with no entitlements file, and nobody had checked
whether a pty survives that. It does, so the pane got built.

### What happened

1. **The signing spike, first and on its own.** A throwaway `pty_spike.rs`
   behind a hidden `dreamd pty-spike`, run out of a real Developer-ID-signed
   bundle from `packaging/build.sh` — `flags=0x10000(runtime)`, `codesign -d
   --entitlements` empty. `openpty` → spawn → write → resize → read all work,
   from the `.app` and again from a session with **no controlling terminal**,
   which is the Finder-launch shape and the case that actually ships. Phase two
   spawns the real `claude`, not `/bin/sh`: a hardened-runtime process forking a
   third-party binary into a pty it owns is the sentence step 5 needed to be
   true. The notarized half was skipped — no local `APPLE_ID`/`APPLE_PASSWORD`,
   and the ticket cannot change runtime capability, only Gatekeeper admission.
   The spike was deleted once the answer was written into `pty.rs`'s module
   docs and into `CLAUDE.md`.

2. **`src-tauri/src/pty.rs`.** One pty per window, created on the pane's first
   open and never at boot. A reader thread emitting base64 chunks and a second
   thread reaping the child, because `wait` blocks and polling `try_wait` from
   the reader would either spin or delay the exit event behind the next read.
   Takes a `Sink` closure rather than an `AppHandle` — the same shape as
   `notify::Notifier`, for the same reason: the tests drive a real pty with no
   window. `Drop` kills, so a closed pane leaves no orphaned `claude`.

3. **Base64 in both directions.** Out, because a 4 KiB read splits a multi-byte
   character and only `Terminal.write`'s stateful decoder is positioned to
   reassemble it. In, because a paste is arbitrary bytes and `btoa` throws above
   U+00FF. Twenty lines of encoder rather than a dependency.

4. **A login shell, not `claude` directly.** A `.app` launched from Finder
   inherits launchd's `/usr/bin:/bin:/usr/sbin:/sbin`, and `claude` installs to
   `~/.local/bin` — spawning it directly works from a terminal and fails from
   the Dock. `PANE_COMMAND` is a `const`, not a template, with a test pinning
   it; tenet 3 in `CLAUDE.md` was extended to name this second shell.

5. **Four commands** (`pty_spawn`/`write`/`resize`/`kill`) under `core:default`,
   no plugin and no capability entry, plus a kill on `RunEvent::ExitRequested`.
   They are the only commands in `main.rs` that emit, and `notify`'s module doc
   now says why that is not a contradiction: terminal output arrives when the
   child feels like producing it, so there is no return value to carry it.
   `marks-changed` is still the only *store* push.

6. **`ui/vendor/`** — xterm.js 5.5.0 and its fit addon, the publishers' own UMD
   bundles from `npm pack`, MIT, with provenance and the upgrade procedure in a
   README. The first vendored JS in the repo, and forced: the CSP blocks a CDN,
   blocks WASM, and blocks inline silently.

7. **The pane itself** — `#pty-pane` as a `flex: 0 0 40%` sibling of
   `#content-scroll`, the same shape as `#find-bar`, so `#workspace` keeps its
   two children and the `body.view-mode` grid rule is untouched. `Ctrl+T`
   (`toggle_pane`, with its `config::Keymap` twin), a titlebar button, ⟳/✕ in
   the header, view-mode and print hide lists, `KEY_ACTIONS` entry. Closing
   hides rather than kills: the conversation survives behind `display: none`.

8. **The macOS app icon**, in its own commit. It was full-bleed 1024×1024 —
   a favicon's convention, not an app's — so dreamd sat visibly larger in the
   Dock than everything beside it. `icon.icns` is now generated from a new
   `src-tauri/icons/macos.svg`, the same four shapes under a
   `translate(100,100) scale(25.75)`, giving Apple's 824×824-in-1024 grid.
   Verified by measuring the alpha bounding box: exactly 824×824 at (100,100),
   against 828×847 at (98,100) for a shipped third-party app on this machine.

### Mistakes & deviations

- **The pane shipped as a keyboard trap, and the plan's guard could not have
  caught it.** The plan says extend `isEditable` so bare letters don't fire
  while the terminal has focus. That was written, then broken to check it had
  teeth — and nothing went red. A capture-phase probe explains why: xterm calls
  `stopPropagation` on every key it handles, so `Escape`, `/`, `n`, `Ctrl+M`
  and `Ctrl+T` reach `document` in the capture phase and none of them in the
  bubble phase, where `wireKeys` listens. The global keymap was never in the
  race — and `Ctrl+T` pressed *inside* the pane did nothing, so the one key
  documented to close it was swallowed by the thing it closes. Fixed with
  xterm's `attachCustomKeyEventHandler`; removing it now turns two `ui-check`
  checks red, one of them showing `FA==` — the stray `^T` leaking to the child.
- **`resizing_a_dead_pty_is_an_error_not_a_panic` names an outcome that does not
  happen.** The master fd is still open, so the ioctl succeeds. Shipped as
  `resizing_a_pty_whose_child_exited_is_not_a_panic`.
- **xterm.js is injected on first open, not declared in `index.html`.** The plan
  said `<script src>` in the document; that costs its parse on every launch,
  including the overwhelming majority that never open a terminal, and 289 KB
  would have landed on first paint. Same argument the plan itself makes for not
  constructing `portable-pty` eagerly. A runtime-created same-origin script tag
  is as CSP-clean as a declared one.
- **The first spike run failed on its own harness**, not on signing: it wrote
  to the master before resizing, so the child raced through to `SIZE_AFTER`
  before the resize landed. The child is the only clock in that script.
- **`cargo test` nearly started a Claude Code session.** The first draft of the
  pty tests called `Pty::spawn`, which runs `PANE_COMMAND`. Split into
  `spawn_command`, and the tests drive `/bin/sh`; a separate test pins
  `PANE_COMMAND`'s shape so the real one is still covered.

### State

`cargo build`, `fmt`, `clippy --all-targets --all-features -D warnings` clean.
205 unit tests (+10 in `pty`), `ui-check.mjs` 108/108 (+20 for the pane),
`config_check`, `theme_check`, `mcp_check`, `marks_check`, `locate_check`,
`node --test ui/paths.test.mjs` 10/10 — all green. Both new frontend guards
proved by breaking them and watching the named check go red.

`/perf-pass` reports 18 regressed and none of it is this change: the baseline
predates step 4, so `marks_loaded` and `d:ipc_get_highlights 14 → 1034 ms` are
step 4's lazy re-anchor appearing for the first time, and the real-app arms are
noise — the *previous* pass run recorded `first_paint` at 10,008 ms against this
one's 2,528 ms. `js_start` flagged at 53 → 58 ms, but historical pass runs sit
at 57–63, so 58 is the norm and the baseline's 53 was the outlier. `first_paint`
improved 5.7%. `/perf-quick` afterwards: 0 regressed. The boot-cost claim does
not rest on any of that anyway — `ui-check` asserts deterministically that boot
loads no vendor script and spawns no process. Release binary 5.76 MB → 5.82 MB,
which is the brotli'd xterm.js.

Open, and named here so it is not lost:

- **The pane has never been looked at.** `ui-check` asserts what the page knows
  — the terminal's buffer model, which IPC calls happen, which classes are set —
  and nothing about what it paints. The layout, the colours and whether `claude`
  actually starts all need `cargo tauri dev` and a human.
- **The Dock icon is likewise unverified visually.** The geometry is measured
  and correct; nobody has seen it in a Dock.
- **The notary's own opinion of a `portable-pty` binary is untested**, for want
  of local Apple credentials. Low risk — it adds no entitlement to reject — but
  it is the one half of the signing question that was not measured.
- **No config key for the pane's command.** It is `claude`, hardcoded. If it is
  not on the login shell's `PATH` the pane shows `[process exited]` and a ⟳.
- **`/perf-deep --update-baseline` is still owed**, from step 4 and now this.

## 2026-07-27 — Step 4: marks that outlive the process, and a pass that found four defects

Step 4 of `docs/plans/agentic-mcp-persistence.md` — wiring thread G's
`marks_file.rs` into the app so highlights, annotations and the stack survive a
quit. It got there. The verification pass at the end found four real defects in
the wiring and two checks that were green for the wrong reason; all six are
fixed in this commit.

### What happened

1. **Startup.** Marks load between `Config::load` and the walk, guarded by
   `has_repo` for the same reason the watcher and the socket are: with no repo
   the root is whatever the walk-up fell back to, and a marks file named after
   it would claim a repo that does not exist. `perf::mark("marks_loaded")` plus
   a `d:marks_loaded` duration. Nothing is re-anchored at boot.

2. **Lazy per-file re-anchor.** `AppState::pending_reanchor` is seeded from the
   loaded marks' distinct file paths and drained by `get_highlights`, which is
   already the first thing `renderCurrent` calls after `innerHTML`. The set is
   what keeps the steady state free — `Store::ensure_reanchored` is idempotent
   but takes source bytes, so calling it unconditionally would re-read the open
   document on every repaint, inside the `save_to_paint` loop. A mark created
   this session was anchored against those bytes and is never in the set.

3. **Debounced save.** A `dirty` flag on the four mutating commands, a 500ms
   thread, and a flush on `RunEvent::ExitRequested | Exit` — which is why the
   builder is now `build()` + `run()` rather than `run()` alone. MCP mutations
   dirty the store by **wrapping the `Notifier`** rather than through a second
   filter: `notify` already fires on exactly the calls that changed something,
   and `mcp/server.rs` has tests holding that line. `reanchor` is the one
   mutation that deliberately does *not* dirty — what it changes is derived
   from the file's current bytes and is recomputed at next boot, so marking it
   would put a marks write on every `:w`.

4. **`adopt_root`.** Flush of the old root, config swap, root swap, load of the
   new root, `dirty` cleared and `pending_reanchor` re-seeded, all in one block
   under the store lock, as the plan required.

5. **Multi-instance.** `cli::repo_is_claimed` — a connect probe against the MCP
   socket, i.e. a *read* of the bind-is-the-lock rule. It lives in `cli` rather
   than `main.rs` for the reason `guard` does: in a `[[bin]]` no test could
   reach the predicate deciding whether a process may write the user's marks.
   A secondary loads marks, never writes, and says so on stderr.

6. **`dreamd marks path|prune [--stale] [--older-than 30d]`.** Bare `prune` is
   a dry run — a destructive verb whose default changes nothing. `--stale`
   never touches an annotated mark; `--older-than` drops answered ones, which
   is exactly the difference between them. Stack pruning reuses
   `marks_file::admit` rather than reimplementing it.

7. **`examples/marks_check.rs`**, 75 checks: modes tightened rather than merely
   created, round trip including `resolved`/`origin`/stack order, five corrupt
   file shapes, the 4 MiB pre-read cap, hand-edited containment, a
   future-version file, an orphaned `.tmp`, two repos, the one-writer lock, and
   `prune` driven through `cli::run`. CI runs it; tenet 2 in `CLAUDE.md` was
   rewritten, because it claimed nothing persists and that is now false.

### Mistakes & deviations

- **`flush_marks` read `persists`, `dirty` and `root` across the store lock.**
  `adopt_root` mutates all three *under* it, so a tick could pass the gate as
  primary on repo Y, block on the lock, and wake up to write repo X's file — as
  a secondary that had just stood down. Now every decision but the cheap
  `dirty` hint is made under the lock.
- **`get_highlights` cleared the re-anchor debt before knowing the read
  succeeded.** `remove(&path).then(read)` drops the path even when the file is
  briefly unreadable — Neovim's atomic-rename save, mid-`git checkout` — leaving
  the marks at last session's line numbers, `Active`, for the rest of the run.
  Now `contains`, with the `remove` after a successful re-anchor and under the
  store lock, which also makes the lock order store → `pending_reanchor` a
  stated invariant rather than an ownership accident.
- **`parse_age` panicked on non-ASCII.** `split_at(len - 1)` lands inside a
  multi-byte character, so `dreamd marks prune --older-than 30é` aborted with a
  backtrace where clap should have printed a usage error. Splits on the last
  *character* now.
- **`prune`'s `--stale` retain had no test.** The three predicates all had real
  teeth; the line that calls them — the one that actually deletes a user's work
  — had none. Swapping `is_stale_droppable(h)` for a bare `h.state == Stale`,
  so `--stale` eats annotated questions, left 197 unit tests and 61 harness
  checks green. `prune` is now driven through `cli::run`, and that break turns
  it red.
- **`truncation_lands_on_a_char_boundary` was toothless, pre-existing.** Its
  comment says "3 bytes each" but `é` is **2**, and `MAX_FIELD_BYTES` is even,
  so the cap always landed on a boundary and a naive `String::truncate` passed.
  Fixture is a 3-byte character now; with the boundary walk removed it panics,
  which is the point.
- **`/perf-pass` was run and thrown away.** 106 of 199 rows moved >15%,
  including pure-Chromium rows that run none of the changed code — Firefox
  media playback at 73% CPU and WindowServer at 97% during the run. Contaminated,
  not a regression. Reported and skipped rather than quoted.

### State

`cargo build`, `fmt`, `clippy --all-targets -D warnings` clean. 197 unit tests,
`marks_check` 75/75, `mcp_check` 46/46, `config_check` 34/34, `theme_check`,
`locate_check`, `node --test ui/paths.test.mjs` 10/10 — all green. Each fix
proved by breaking the guard and watching the named check go red.

Open, and named here so it is not lost:

- **No perf numbers for this change.** `/perf-pass` and
  `/perf-deep --update-baseline` both still owed, on a quiet machine. The only
  figure that survived the contaminated run is `d:marks_loaded` at 0.042 ms —
  under the plan's 1 ms, but on the *empty* load path, since neither corpus repo
  has a marks file. A seeded measurement has not been taken.
- **Both manual checks are unrun.** Quit-and-reopen (marks, stack and badge at
  boot) and a second window announcing itself as secondary need the GUI, which
  cannot be driven from this environment.
- **The MCP read tools never re-anchor.** The plan is explicit that `get_stack`
  needs it; `mcp/tools.rs` has no `ensure_reanchored` call, and the socket
  thread cannot see `pending_reanchor` — it lives in `main.rs`'s `AppState`. So
  the first `get_stack` of a cold session hands the agent line numbers from the
  previous session, which costs an incorrect edit rather than a cosmetic
  misplacement. Out of this thread's scope; the highest-value follow-up.
- **A secondary window is silent in the GUI.** The plan asks for an indicator.
  `repo_info` exposes `persists` and nothing in `ui/app.js` reads it, so a
  second window looks identical to the first while losing every mark on quit.
- **The cap is enforced at load, not at save**, and `marks.max_per_repo` does
  not exist in `config.rs`. A session can write 2500 marks and the next boot
  silently drops 500 with no user action in between.
- **A secondary never reclaims.** `persists` is re-decided only when the root
  moves, so a window that started second stays a non-writer even after the
  primary quits.
- **Step 4's `main.rs` wiring has no automated coverage at all** — deleting
  `state.touch()` from `set_annotation`, or gutting `flush_marks`, leaves every
  suite green. Expected for a `[[bin]]`, but `flush_marks`'s gate policy and
  `marking_dirty` are logic, and CLAUDE.md's own rule says logic goes where a
  test can reach it.

## 2026-07-27 — Step 3c: the socket, the shim, and an independent pass that found teeth missing

Step 3c of `docs/plans/agentic-mcp-persistence.md` — the transport between the
protocol core (3a) and the frontend listener (3b), both of which had landed and
neither of which could reach the other. Three commits: `e1ecefe`, `3322787`,
`eaa86d0`. It got there; the interesting half of the thread was the verification
pass, which found two of the security checks written that same hour were green
for the wrong reason.

### What happened

1. **`/perf-quick` first, on the three new pure modules.** Nothing beyond noise:
   `locate_single/*` +5-7% and `reanchor_today/*` +8-10%, all `!` rather than
   `XX`, reproducible across two runs, same family, and no new code on those
   paths. Reads as code layout, the pattern
   `rust-bench-moves-from-code-layout` already records.

2. **`Store::resolve`, committed alone.** `annotations.rs` was frozen for the
   parallel window, so thread C reached `resolved` through the only public seam
   it had — `into_parts`/`from_parts` — which resets `reanchored`. Correct, but
   it bought a wasted `SourceIndex` rebuild per resolved mark, in the primary
   loop, where an agent closes marks in bursts. The window is closed, so the
   capability landed where it belongs: `Store::resolve` records the answer and
   drops the stack entry in one pass and leaves the reanchor gate alone.
   `tools::with_highlight_mut` deleted. Proved with teeth: adding
   `self.reanchored.clear()` turns
   `resolving_a_mark_does_not_invalidate_the_reanchor_cache` red (line_start
   2 → 4).

3. **The wiring.** `mcp/server.rs` — a Unix socket under
   `~/.config/dreamd/run/`, mode 0600, named by the same FNV-1a root hash
   `marks_file` uses. Binding it is how a dreamd claims a repo: `AddrInUse` that
   *connects* means a live owner and this process runs as a secondary; one that
   refuses is a crash leftover, unlinked and rebound. `mcp/shim.rs` — `dreamd
   mcp`, answering `initialize`/`tools/list` from the compiled-in schema const
   and proxying only `tools/call`, because Claude Code caches the tool list for
   the session and a dreamd that happened to be closed at client startup would
   otherwise mean no tools until the next restart. `notify.rs` — `marks-changed`,
   emitted only from the MCP layer, with the server taking a `Notifier` closure
   rather than an `AppHandle` so the whole transport runs headless. `main.rs` —
   `Arc<Mutex<Store>>`, an `mcp_cancel` slot beside `watcher_cancel`, spawn in
   `.setup()`. `cli.rs` — `Cmd::Mcp`, routed before the config read the other
   subcommands need.

4. **The `adopt_root` hazard, with one thing the plan did not anticipate.** The
   socket retires and re-binds in the block that already retires the watcher, as
   specified — but guarded on the root having actually moved. The retiring
   thread only notices its cancel flag on the next accept poll, so re-adopting
   the *same* root would rebind onto a path the old server was still listening
   on, read that as another live dreamd, and stand down as a secondary: MCP
   silently gone for the rest of the session.

5. **`examples/mcp_check.rs`**, because the socket lives under `config_dir()`
   and `cargo test` may not go there. It drives the real transport, and for the
   tool calls it drives them through `shim::forward` — the exact wire an agent's
   calls travel — rather than through a hand-rolled client.

6. **An independent verification pass over steps 2 and 3**, in a fresh context,
   with the brief "break the guard, watch the named test go red, restore,
   report". Five checks stayed green with their guard removed; see below.

7. **The two in this session's own code were fixed** (`eaa86d0`), each verified
   by breaking the guard again and confirming red.

### Mistakes & deviations

- **Three `mcp_check` containment checks pinned nothing.** All three survived
  deleting `guard::inside_root` outright. `resolve_in_root` refuses for three
  different reasons — the path will not canonicalise, the path is outside the
  root, the quote is not in the file — and the fixtures were tripping the other
  two: `../../etc/passwd` resolves under the *temp* directory rather than under
  `/`, so those targets did not exist, and no target contained the quote. Now
  real files outside the root containing the quote, and each row also asserts
  the refusal message says "outside the repo root". Guard off → six failures.
- **`a_refused_write_emits_nothing` was named for a check it did not need.** Its
  `mark_passage` path is double-gated — a refused call has no
  `structuredContent.id` to read either — so it stayed green with the `isError`
  early-out deleted. The `resolve_highlight` arm is the one where the flag is
  the only gate, because the id comes from the caller's arguments and is just as
  readable on a failure. Added `resolving_a_refused_call_emits_nothing`; the doc
  comment now says which arm is which instead of claiming both.
- **`/perf-pass` flagged `save_to_paint` and it was the baseline, not the
  change.** +16%, then +35%, then unflagged, across three runs of one unchanged
  binary. A direct A/B settled it: `loop.sh`, three reps per arm, p50
  3217/3249/3232 at `d2ca8a8` against 3215/3686/3206 at HEAD — flat. Run at
  `bc79844`, the baseline's *own* commit, it gave 3790 and 3207 against the 2377
  recorded there, so that row does not reproduce at the commit it was taken
  from. Noted in memory rather than acted on; the row is also one sample, since
  p50 equals p95 every run.
- The plan calls step 3 a clean abort point. It is not, quite:
  `mcp/server.rs` imports `marks_file::root_hash` for the socket name, so it
  will not compile without step 4's module.

### State

`cargo build`, `fmt`, `clippy -D warnings` clean. 194 unit tests, `mcp_check`
46/46, `config_check` 34, `theme_check`, `ui-check` 88/88,
`node --test ui/paths.test.mjs` — all green. CI gained
`cargo run --example mcp_check`.

Open, and named here so it is not lost:

- **The manual acceptance test has not been run.** The GUI cannot be driven from
  this environment. Everything below the window is verified; the queue-first
  loop end to end — three marks, one prompt, badge 3 → 2 → 1 → 0 without
  touching the window — is not.
- **Three toothless guards left standing, all outside this session's scope.**
  `untrusted.rs`'s `the_sentinel_differs_between_processes` passes with
  `sentinel()` replaced by a hardcoded literal — it never asserts the value
  derives from `process_seed()`, which is the property the plan bolds. In
  `ui/app.js`, `contentEl.normalize()` and both halves of `repaintHighlights`'
  find bracket are individually deletable with `ui-check` still at 88/88.
- **`adopt_root`'s socket retire+rebind has no test of any kind**, including the
  `previous_root != root` guard above. It lives in un-importable `main.rs` —
  the one place step 1's "lift it out so a test can reach it" discipline was not
  applied.

## 2026-07-27 — Step 1 of the agentic plan: opaque ids, and a frozen Store

Step 1 of `docs/plans/agentic-mcp-persistence.md`, implemented in full including
the parts whose consumers arrive two steps later. One commit, `af2c575`. It got
there, and the interesting half of the thread was chasing a perf row that turned
out to be an artifact.

### What happened

1. **`Highlight.id` was a `u64` from a counter that restarted at 1 every
   process.** So an id held across a restart addressed a *different* highlight —
   silently, which is the worst way to be wrong, and the blocker for every later
   step (an agent holds ids across its own lifetime, and step 4 persists them).
   Ids are now `h` + 16 lowercase hex, FNV-1a over wall-clock nanos, the pid, a
   per-process `AtomicU64` and one OS-seeded value (`RandomState`'s keys, so no
   new dependency). `process_seed()` is `pub(crate)` because step 2's
   untrusted-content sentinel needs the same entropy and shouldn't mint its own.

   **Opaque rather than numeric** so nothing downstream can do arithmetic on an
   id or read ordering out of one — the only order dreamd exposes is the stack's,
   which is the human's asking order. **Not content-derived**: two identical
   quotes in one file would collide, and re-highlighting deleted-then-restored
   text should mint a new identity rather than resurrect the old one.

2. **Anchoring moved out of `main.rs` into `Store::add_anchored`.** The
   `add_highlight` command read the file, called `markdown::locate`, and defaulted
   to `(0, 0)` at the command layer — and `main.rs` is a `[[bin]]`, so that
   fallback had no test at all. Same situation, and the same fix, as the header
   comment in `guard.rs` describes. `an_unlocatable_quote_still_becomes_a_highlight_at_line_zero`
   now pins it.

3. **The whole `Store` shape was frozen in this one commit**, not just the ids:
   `Origin` (`Human`/`Agent`), `Resolution`, `Deserialize` with container-wide
   `#[serde(default)]` and no `deny_unknown_fields`, `from_parts`/`parts`/
   `into_parts`, and a `reanchored: HashSet<String>` behind `ensure_reanchored`.
   None of those have a reader yet. That is the point: `annotations.rs` is the
   worst file in the repo to merge, four threads are about to run against it in
   parallel, and one extra hour here saves two merges later. The file is closed
   until step 5.

   `Deserialize` deliberately stops at `Highlight` and does **not** reach `Pair` —
   `Pair` is a projection, not a stored shape, and deriving it would invite
   somebody to persist it.

4. **`locate_check` joined CI**, keyed on a cached corpus. It had been excluded as
   "a perf tier's job", which was right until anchoring moved: 611 fixtures with
   an independent oracle are the only thing that would catch the move breaking it.
   The cache key covers `gen.mjs` as well as `manifest.json`, because a changed
   generator makes different fixtures for an unchanged manifest.

5. **The frontend needed exactly two lines.** Both `Number()` calls on
   `dataset.id` dropped; everything else already round-tripped ids as strings.
   The perf harness fixtures and the `ui-check.mjs` stub now hand over
   `h`-prefixed ids so a numeric-id assumption can't pass unnoticed.

### Mistakes & deviations

- **One deliberate deviation from the plan.** The plan's `add_anchored` signature
  omits `origin`, but the same section says step 3's `mark_passage` calls it with
  `origin: Agent` while `annotations.rs` stays closed. Those can't both hold, so
  `add_anchored` takes an `Origin`. Still 7 arguments, so clippy is unbothered.

- **Chased a reproducible 7% perf regression that wasn't one.** `/perf-quick`
  flagged all four `locate_single` cases. Interleaved A/B against a stashed tree
  reproduced it cleanly — clean 6.15/6.15 ms, dirty 6.55/6.61/6.58 — which is
  normally where you stop and believe it. But the rows are in `markdown::locate`,
  which this commit does not touch. Stubbing out `add_anchored`'s call to
  `locate` (the only new in-crate caller) changed nothing. The isolation that
  settled it: on a clean tree, adding **only** `origin` and `resolved` to
  `Highlight` — no id change, no `HashSet`, no new function — reproduces the
  entire +7%, in a function that never sees a `Highlight`. Everything in
  `src-tauri/` links into one bench binary, so perturbing any module repartitions
  codegen units and shifts alignment in a memchr-heavy loop. Layout, not cost.
  Saved to memory, because "reproducible" was not enough to make it true.

### State

Green: `cargo fmt --check`, `cargo clippy --all-targets --all-features -D
warnings`, `cargo test --all-features` (105, up from 99), `cargo build`,
`node --test ui/paths.test.mjs`. Plus `cargo run --release --example
locate_check` (exit 0; 0 wrong with context, 0 moved across 611 fixtures) and
`node perf/harness/ui-check.mjs` (79/0).

`an_id_from_a_previous_session_resolves_to_nothing_rather_than_the_wrong_highlight`
was verified to have teeth: the `u64` counter was temporarily restored and it was
the only one of the 24 `annotations` tests to fail, on exactly its own assertion.

`/perf-quick`: `apply_highlights` did not move, which is what the plan named as
the tell that dropping `Number()` had broken DOM keying. The `locate_single`
rows are the artifact above. `perf/baseline.json` untouched.

Open: steps 2–5 of the plan. `annotations.rs` is frozen until step 5 — a thread
that wants to edit it is a signal this shape was wrong, not a licence to change
it.

## 2026-07-27 — Three things that were never actually being run

A scoped session — `.github/workflows/ci.yml`, `perf/harness/ui-check.mjs`,
`src-tauri/src/send.rs`, nothing else — run from Linux, which is the whole
caveat on it. It got there: one commit, `fc549cc`. The thread's shape turned out
to be a single theme rather than three tasks. Every one of the three was code
that existed, looked maintained, and was not being executed by anything.

### What happened

1. **Linux bring-up first.** `apt-get update` failed 403 on two unrelated PPAs
   (`deadsnakes`, `ondrej/php`) that the image carries, which takes the whole
   update down with them. Moving those two `.sources` files aside, updating,
   installing `libwebkit2gtk-4.1-dev` + `libgtk-3-dev`, then moving them back is
   the fix. Worth knowing: `-o Dir::Etc::sourceparts=/dev/null` is *not* the
   shortcut it looks like — `ubuntu.sources` lives in `sources.list.d/` too, so
   that drops the main archive along with the broken PPAs and every package goes
   "unable to locate".

2. **`send` leaked a temp file per send, forever.** `write_temp` wrote
   `dreamd-query-<pid>-<n>.md` and nothing ever deleted one. Tenet 1 is exactly
   why: every write goes outside the repo, so no repo-level cleanup was ever
   going to reach them. The name now carries a day stamp —
   `dreamd-query-<day>-<pid>-<n>.md`, `epoch_day` being seconds/86400, no
   calendar crate — and the first send of a session sweeps anything older.

   Two decisions inside that. **Today's are kept**, including another process's:
   the path in a `read @<file>` prompt has to stay readable for as long as the
   agent might act on it, and a concurrent dreamd is still pointing at its own.
   And the sweep compares `<`, not `!=`, so a clock that has run backwards leaves
   a file alone rather than deleting a live one. `query_day` requires all three
   name fields to be present, which is what stops a pre-stamp `<pid>-<n>` name —
   whose first field parses perfectly well as a day — from being deleted on a
   misread. Those files are left behind instead; a one-time wart, chosen over
   deleting on a name shape we can't confirm we wrote.

3. **`ui-check.mjs` had run on exactly one machine, ever.** `const UI =
   "/Users/oliverfong/toadmountain/dreamd/ui"` — an absolute path into one
   laptop's checkout. The frontend's only correctness harness, 79 checks, and it
   could not execute anywhere else. Derived from `import.meta.url` now. Nothing
   else needed changing, and that is the interesting part: all 79 pass unaltered
   on Linux/Chromium, which is the first actual evidence for the claim in its own
   header that what it asserts is DOM structure and IPC rather than anything
   engine-specific.

4. **CI now runs both of the things it was only compiling.** `clippy
   --all-targets` type-checks the four examples and stops there, so a harness
   could go red for weeks with nobody running it. The rust job gained
   `config_check` + `theme_check` (34 and 10 checks, ~7s combined, and
   `config_check` points `XDG_CONFIG_HOME` at a scratch dir before touching
   anything). The ubuntu job gained the frontend harness — `npm ci` + `npx
   playwright install --with-deps chromium` + `node ui-check.mjs` — with the
   timeout 5 → 15 to cover the browser download, and an npm cache keyed on
   `perf/harness/package-lock.json`. `locate_check` was deliberately left out: it
   wants the ~11MB generated corpus and a release build, which is a perf tier's
   job, not a per-push one.

### Mistakes & deviations

- **Wrong branch, briefly.** The session opened on
  `claude/tauri-deps-send-tests-5mqrrh` with the harness insisting on it, while
  the instruction and CLAUDE.md both say straight to main. The first `git fetch`
  made `origin/main` look ~100 commits stale, which read as "main is abandoned,
  this branch is the real line" — it was just a stale ref. A fetch mid-session
  (prompted by the user pushing `5785a4d`) showed main was the tip all along and
  the branch was one commit *behind* it. Moved onto `origin/main` and committed
  there. No overlap with the two doc files in `5785a4d`.
- **Read `cargo build` as passing when it had not.** The warm-up ran as
  `cargo build 2>&1 | tail -15` in the background; the pipeline's exit code is
  `tail`'s, so it reported 0 while the bin had failed to compile. Caught on the
  first real `cargo test`. Nothing downstream depended on the wrong reading, but
  a piped exit status is not the command's.
- Spent a while inferring what `ci.yml` was in scope *for*, since only the
  `send.rs` test name was specified. The ui-check path fix is what settled it —
  fixing the path and not then running it in CI leaves it exactly as unverified
  as it was.
- Local Playwright resolved to 1.61.1 (wants Chromium 1228) against the image's
  pre-installed 1194. Pinned `playwright@1.56.1 --no-save` for the local run
  rather than downloading a browser or touching `package.json`, which is out of
  scope. Note for later: `package.json` floats `^1.49.0` while the lockfile pins
  1.61.1 — harmless for CI, since `npm ci` and `playwright install` agree with
  each other, but it is why the local run needed pinning at all.

### State

Committed `fc549cc`, pushed to `main`. Three files.

**The green suite here is strictly smaller than what CI runs, and this session
could not close that gap.** On Linux `main.rs` does not compile —
`#[cfg(target_os = "macos")]` gates out `menu.rs` and the NsFileManager trash
context — so `cargo build` and `cargo test --all-features` both fail on the bin
target, and `clippy --benches`/`--tests` fail with them because cargo builds the
bin for those. Pre-existing and untouched, but it means the bin, the benches and
the bin's test target went unchecked; macos-14 is the first real look at them.

Verified: `cargo fmt --check`, `cargo clippy --lib --examples --all-features -D
warnings`, `cargo test --all-features --lib` (94 passed, including the two new
`send` tests), `cargo build --lib`, `node --test ui/paths.test.mjs` (10),
`node perf/harness/ui-check.mjs` (79), `config_check` (34), `theme_check` (10),
and `npm ci` against the committed lockfile in a scratch copy.

No `perf-pass`: it drives the real app and the macOS timing tools, neither of
which exists here, and the `src-tauri/` edit is on the send path — one `read_dir`
per session, nowhere near render, locate or search. The new CI steps themselves
are unproven *in CI*; the commands were verified locally, not on a runner. Still
open from the previous session: the baselined Chromium scroll regression, which
this thread did not touch.

## 2026-07-27 — Deep perf run, and a scroll regression frozen into the baseline on purpose

A one-task session: run the deep tier, move `perf/baseline.json`, commit it. It
got there, and turned up one thing worth more than the baseline itself — a real,
reproducible Chromium scroll regression that arrived with the overnight feature
batch and had been invisible because every tier since was diffing against
pre-feature numbers.

### What happened

1. **The baseline was two days and one feature batch stale.** It sat at
   `312ac8b` (25 Jul). Everything since — the outline panel, `/` search, marks,
   code-block copy buttons, the reading rail, the print stylesheet — landed
   against it, so `perf-quick` and `perf-pass` had been comparing current code
   to a pre-feature zero the whole time.

2. **Ran `./perf/run.sh deep --update-baseline` on a quiet machine.** load1 1.71
   on 8 cores, against 3.03 when the old baseline was taken. All four tools
   present (`hyperfine`, `samply`, `cargo-bloat`, `xctrace`), no section null,
   nothing silently skipped. 276 metrics compared. New baseline is
   `bc79844`, committed as `85c4963` with the before/after in the body.

3. **Criterion came back all green** — `reanchor_today/500` 311.6 → 281.6 µs,
   `reanchor_exact_source/10` −11.8%, `keystrokes/10` −15.3%, `render/mixed/8k`
   −9.6%, nothing regressed. Recorded as a re-zero, not a win: the machine was
   quieter, and no commit in the range claims a core optimization.

4. **The Chromium scroll rows are a real regression, and it was frozen in
   deliberately.** `mixed-2m` `renderer.total_ms` 63.6 → 85.8 (+34.9%),
   `main_thread_task_ms` 240 → 335 (+39.5%), highlightMode 285 → 434 (+52.2%).
   It reproduces across two independent deep runs today at load 4.54 and 1.71,
   so it is not noise. `script_ms`, `style_ms` and `layout_ms` are all still 0
   while raster and composite carry the whole delta — which points at declared
   CSS rather than added JS, matching the earlier finding that a rule can cost
   double digits by merely existing. Baselining it means it stops being flagged,
   so it is named in the commit body and here instead. It needs its own A/B
   against the new zero; that is the next perf task, not this one.

5. **Read the profiles rather than the summary table.** Render is 84% syntect
   `highlighted_html_for_string`, of which 75% of total time is
   `syntect::parsing::regex::Regex::search` down into fancy_regex's VM, plus ~6%
   first-use regex compilation — pulldown-cmark itself is noise, so a render win
   has to come out of syntect. `repo-5000` startup is 43% `opendir`/`open`, 18%
   `stat`, 8% `getdirentries64`: the `ignore` walker's syscalls with nothing
   above them, consistent with `walk_done` being 46.9 ms of the 53.6 ms total.

6. **Two smaller shifts recorded.** A new `d:decorate_code` phase mark at 8 ms
   (the code-block copy buttons), and `d:ipc_get_highlights` swapping scenarios —
   the ~1 s seeded cost now lands on `launch_small_repo` (3 → 923 ms) instead of
   `launch` (1002 → 14 ms). Both of today's runs agree, so the new baseline is
   self-consistent and the total is unchanged.

### Mistakes & deviations

- **`profile.symbolicated: true` overstates what the tier delivered.** samply's
  capture symbolicates properly against `target/profiling/examples/render_doc`,
  but the Instruments trace profiles `/Applications/dreamd.app` — the release
  build, which sets `strip = true` — so every app frame in it is a raw address.
  Only the dyld and libc frames resolve. The startup finding above survives that
  because it is a syscall finding, but the flag should not be read as "both
  profiles are readable".
- Nearly reported the samply profile as unsymbolicated too: its saved JSON holds
  raw addresses because samply defers symbolication to `samply load`. Resolved
  them with `atos` against the profiling binary instead of concluding the tier
  had failed.

### State

Committed `85c4963`, `perf/baseline.json` only — the two untracked files under
`docs/` belong to another session and were left alone. No Rust or `ui/` change
this session, so no `cargo build` gate and no `perf-pass`; the deep run *is* the
measurement. Open: the Chromium scroll regression, unexplained and now
baselined. Chromium numbers throughout are relative signal, not WKWebView.

## 2026-07-27 — CI from nothing: 99 tests, the security tenets made reachable, and one cache reverted for being useless

The ask was to improve CI/CD ahead of two pending decisions — whether highlight
and annotation persistence happens at all, and how an MCP server folds in — by
locking down the parts of the app that are settled either way. It got there:
`ci.yml` now runs on every push and PR, `cargo test` runs 99 tests where it ran
zero, and five of the six security tenets went from no automated verification to
tests that were each proven to fail when the guard is removed.

### What happened

1. **The starting position was worse than it looked.** `release.yml` was the only
   workflow — `v*` tags, `release: published`, `workflow_dispatch`. Nothing ran on
   main, nothing on a PR. No `rustfmt.toml`, no `clippy.toml`, no `[lints]`. Not
   one `#[test]` in the repo. Of the six tenets, exactly one had coverage: the
   `.dreamd.toml` `theme_css` refusal, in `config_check.rs`.

2. **Two enforcement points were structurally unreachable.** `open_external`'s
   scheme allowlist and `delete_file`'s repo-root containment lived in `main.rs`,
   which is a `[[bin]]` — no test, example or bench can import it. So they moved
   to a new `guard` module (`allowed_scheme`, `inside_root`) with the commands
   keeping the I/O and calling in. That is a prerequisite, not a cleanup: without
   it the tenets are enforced by code nothing can assert on.

3. **`ui/paths.js`.** `normalizePath` and `insideRepo` — the frontend half of the
   same tenet, for relative links and images — came out of `app.js` into a second
   classic script, `defer`, loaded first. Not an ES module: the frontend has no
   build step, and `script-src 'self'` already permits a second *file* (only
   inline script is blocked). `insideRepo` takes `repoRoot` as an argument now;
   two call sites in `interceptLinks` pass it. `ui/paths.test.mjs` drives it under
   `node --test` with zero dependencies, via `node:vm`, so what it tests is
   literally the file the webview loads.

4. **99 tests.** 89 Rust (`guard`, `markdown`, `theme`, `annotations`, `config`,
   `fs_walk`, `lib`) plus 10 node. Deliberately scoped to what will outlive the
   persistence and MCP decisions. `annotations::Store` got particular attention —
   id monotonicity across removals, `set_annotation` as the thing that enqueues,
   stack order under re-annotation, Stale-not-dropped — because it is the exact
   surface a persistence layer would wrap, so pinning it now makes that decision
   cheap. Nothing touches `config_dir()`: that reads the real `~/.config/dreamd`,
   and only a whole process can sandbox it, which is `config_check`'s job.

5. **Each guard was proven to have teeth.** A green assertion says nothing about
   what it would catch, so every security fix was reverted in a *plausible* way
   and the matching test confirmed red: the allowlist widened to `Some(_)`,
   `inside_root` made a textual `starts_with`, the `Event::Html` arm passed
   through unescaped, `user_path`'s `..` check dropped, `insideRepo` made a plain
   `startsWith`. Five for five, tree restored each time.

6. **`ci.yml`.** fmt, `clippy -D warnings` over `--all-targets --all-features`,
   `cargo test`, then a real `cargo build` — clippy is a check, not a link. On
   **macos-14**, which is not a preference: Tauri on Linux needs webkit2gtk, and
   the `#[cfg(target_os = "macos")]` paths compile nowhere else. Runners are free
   on a public repo. `pull_request` is included even though commits go straight to
   main — it costs nothing unused and means an agent-authored branch is checked
   before it lands. Toolchain **pinned** to 1.97.1: `-D warnings` against
   `@stable` turns main red on a lint nobody wrote, and a clippy disagreeing with
   the local one is worse than no clippy.

7. **Verified on GitHub, not just locally.** PR #6 green on a real macos-14
   runner, main green twice, two `workflow_dispatch` release rehearsals green on
   both arches with `release` and `tap` skipped as designed — nothing published.

### Mistakes & deviations

- **The `tauri-cli` cache was added on a false premise and reverted the same
  session.** The claim — inherited from exploration and repeated without checking
  — was that `cargo install tauri-cli` recompiles on both matrix legs of every
  release because `Swatinem/rust-cache` covers `./target` and not `~/.cargo/bin`.
  It covers `~/.cargo/bin` too. The tell came from insisting on measurement: on
  the first dispatch the new cache step *missed*, and `Install tauri-cli` still
  finished in **1 second**, which is only possible if the binary was already
  restored. The 6m10s/11m30s the step cost on 2026-07-26 was a cold rust-cache,
  not a per-run cost. Two rehearsals, both arches, no difference. Removed in
  `c2c6a48`, with the reasoning written into both the workflow and CLAUDE.md so
  the same wrong inference isn't drawn from the same evidence next time.
- **Three test fixtures were wrong before they were right**, each corrected by
  reading the code rather than adjusting the expectation to match: a hint test
  that assumed `best_match` uses the hint with no context (it short-circuits to
  `find`), an off-by-one on which line a repeated block starts, and a `build_tree`
  case that forgot `Path::components` yields a `RootDir`. The last two now assert
  the real behaviour, and the first grew a second test documenting the
  short-circuit explicitly.
- **rustfmt mangled one comment** — it aligned the `blocks` explainer in
  `render_with` against the trailing `// (lang, text)` above it, indenting it 50
  columns. Fixed at the source by moving the trailing comment to its own line,
  rather than accepting the output or adding a `#[rustfmt::skip]`.
- **The perf tier's red rows were not a regression, and saying so took work.**
  Against `baseline.json` the pass run reported 20 regressed / 35 slower. But the
  baseline is `312ac8b` from 2026-07-25, and the correct comparison is this
  morning's deep run at `f1d21d2`. Against that, the metric that matters —
  `real.loop.debug-h10.save_to_paint_ms.p50/p95` — is **26% faster** (3211 →
  2359). The residual cluster was Chromium-only, so it got the direct A/B those
  rows require: three reps per arm, current `ui/` vs pre-session `ui/`. Current
  came out *faster* (47.0 vs 50.6 ms innerHTML p50) with per-rep spread of
  44.7–54.4, i.e. the reported +20% is run-to-run variance, not the `paths.js`
  split.

### State

Green everywhere. `cargo fmt --check`, `cargo clippy --all-targets
--all-features -D warnings`, `cargo test --all-features` (89 passed), `cargo
build`, `node --test ui/paths.test.mjs` (10 passed), and `node
perf/harness/ui-check.mjs` (79 passed, 0 failed) all clean locally; `ci` green on
main. Five commits pushed: `83569da` fmt/clippy, `eaed55a` the Rust tests and
`guard`, `fa88a06` the `ui/paths.js` split, `89eb83e` `ci.yml`, `c2c6a48` the
cache revert.

Perf: no regression attributable to this session (see above). `perf/baseline.json`
untouched — it is two days stale at `312ac8b` and now disagrees with the tree
enough to report phantom rows (`d:decorate_code` shows as a brand-new metric, and
`d:ipc_get_highlights` as a 98.7% "improvement"). **Refreshing it needs a
deliberate `perf-deep --update-baseline`, which was not run.** Results file:
`perf/results/pass-c2c6a48-20260727-124328.json`.

Left open, deliberately out of the chosen scope: the three `examples/*_check.rs`
harnesses and `ui-check.mjs` still do not run in CI. `config_check` and
`theme_check` are two `cargo run --example` lines against an already-compiled
crate; `locate_check` needs the 22 MB corpus cached on the sha256 of
`perf/corpus/manifest.json`; `ui-check.mjs` needs Playwright *and* a fix to the
hardcoded absolute path at `perf/harness/ui-check.mjs:19` before it runs anywhere
but this machine. Also untested: `search`, `catalog`, `watcher`, `cli`, `menu`,
and everything in `main.rs` that isn't a lifted predicate.

## 2026-07-27 — find reshaped around Enter, the regex toggle deleted, and three goes at one stale-paint bug

Follow-up to the session below, all of it driven by the author using the feature
in the real app. The search model changed shape, the regex button came out, and a
lingering-highlight bug took three attempts because the first two were fixing the
wrong layer.

### What happened

1. **Nothing searches until Enter.** The `input` handler, the 60ms debounce and
   its guessed constant are gone; `commitFind` is now the only function in
   `app.js` that searches. Painting per keystroke flickered the highlight
   through the prefixes of the word being typed and yanked the pane to a new
   match mid-word — and it was where the author's reported glitches lived. The
   fix deletes the code path rather than patching it. `findQuery` now means "the
   last *committed* pattern", which is what lets the box be edited freely
   without disturbing a live search.

2. **Enter keeps the bar open and blurs the input.** Previously it closed the
   bar. The blur is load-bearing: focus in the input means `isEditable` swallows
   every bare-letter binding, so `n` would type an `n`. Re-committing an
   unchanged pattern steps instead of re-searching.

3. **Bar visible ⇔ highlights visible, as one rule.** Escape and the ✕ button are
   the same exit and both clear pattern, match set and paint. The `:nohlsearch`
   state added earlier in the day — bar closed, matches still lit — is gone with
   `findLit`. There is no longer a way to be looking at highlights with nothing
   on screen explaining them, and no second command to learn.

4. **The `.*` toggle is deleted; the pattern is interpreted instead.** Literal
   first, re-read as a regex only where the literal finds nothing. Chosen over
   plain-regex because plain-regex has a silent-wrong-answer mode: `app.js`
   would also match `appXjs` with nothing on screen admitting it. Under this
   rule `.` finds the one literal dot rather than all 2000 characters, while
   `\bread\b` and `(one|two)` fall through to the regex the reader plainly
   meant. A lone `(` is simply a literal, so **the invalid-pattern state stopped
   existing** and the red `.invalid` border and "bad pattern" branch came out
   with it. `#find-mode` shows `regex` when the fallback fires, so it is never
   silent.

5. **The stale-paint bug, in three attempts.** Matches stayed on screen after the
   bar closed and vanished one at a time as clicks and scrolling repainted each
   strip.

   - *First attempt:* `CSS.highlights.delete()` was already being called and the
     model was verifiably empty. Replaced delete/re-set with two `Highlight`
     objects registered once and mutated thereafter. Better API usage, and it
     helped, but not enough.
   - *Second attempt:* collapse every range before clearing, on the reasoning
     that a live `Range` inside a `Highlight` is tracked by the engine. Cleared
     most matches — and the author came back with a photo of one fragment
     lingering a line away from its word.
   - *Third, and the actual fix:* both earlier attempts ask the engine to derive
     a dirty region from a **geometry** change, which it does unreliably; the
     displaced fragment was the tell. Now `#find-css` is emptied **while the
     ranges are still registered and painted**, so their style becomes "no
     highlight" — the path a theme switch takes, which repaints exactly the
     regions the engine already knows it coloured. The model is torn down after,
     since the dirty regions are recorded by then. The collapse stays as a belt,
     demoted and commented as such.

   Falling out of it: the `::highlight()` rules now exist only *while a search is
   live* rather than from the first `/` to the end of the session, so the +27%
   per-render cost found in the session below is avoided in more cases than
   before.

### Mistakes & deviations

- **Two failed fixes before the right one, both at the wrong layer.** Worth
  recording precisely because both were locally reasonable and both were
  confirmed working by the test suite. Geometry-derived invalidation is the
  wrong lever; style withdrawal is the right one.

- **The harness assertion could not see the bug, and the first replacement could
  not either.** It asked whether the registry entry existed — the one question
  that returns "clean" while stale pixels are on screen. The first rewrite asked
  whether any uncollapsed range remained *after* closing, which is also blind:
  clearing empties the highlight either way, so there is nothing left to inspect.
  The version that works snapshots the painted `Range` objects onto `window`
  **before** closing and asks afterwards whether they were collapsed. Both new
  guards were then verified to fail against the code they replaced — `3 stray`
  for the collapse, `214 chars of css` for the style withdrawal.

- **One test expectation was wrong rather than the code:** `findSpans("a.c", "abc
  abd")` was asserted to find two regex matches. `a.c` does not match `abd`.
  Corrected the fixture, not the implementation.

### State

`perf/harness/ui-check.mjs` **79 passed, 0 failed**, up from 73 across the
session. The pure-function harness is 15/15 and now covers the literal-first rule
directly, including the `.`-must-not-be-a-wildcard case. No Rust changed this
round, so no `cargo build` gate; perf tiers skipped at the author's request.

The stale-paint fix is **verified only in Chromium, where the bug never
reproduced** — it is a WKWebView invalidation gap, and the author confirmed the
final version works in the real app. That confirmation is the only evidence for
it that exists; nothing in this repo can regression-test it.

Still open from the session below: `FIND_MAX_HITS = 2000` is a guess, and placing
a highlight over a painted match has not been checked by hand.

## 2026-07-27 — reading progress cut, `/` content search built, and a CSS rule that cost 27% by existing

The reading-position indicator came out at the author's request, and
`docs/plans/vim-style-content-search.md` was built as designed. Both landed. The
session's one real finding was not in either: declaring a `::highlight()` rule
costs a large, measurable amount on every render whether or not anything is ever
highlighted.

### What happened

1. **Removed the reading progress indicator** (`5e2cad1`, one day old).
   `#progress-pct`, `#progress-rail`, `#progress-fill`, `measureProgress` and its
   four call sites, the passive scroll listener and the `ResizeObserver` in
   `wireUi`, the print hide-list entry, the `body.view-mode` comment explaining
   why the rail stayed, the harness's "view mode keeps the progress rail" check,
   and the README bullet. `docs/plans/reading-progress-indicator.md` and the idea
   log were left as history.

2. **Built `/` content search**, following the plan's recommendations rather than
   the idea file's:

   - **Haystack is the flattened *rendered* text**, not the markdown source. The
     plan had already overturned that decision and the code confirms why: the
     source is never resident in the frontend, and `scanTextNodes` +
     `nodeIndexAt` — built for `applyHighlights` — already do the whole
     offset→DOM job. `te**s**t` matches the way the reader sees it; `](` matches
     nothing. Index built lazily on first search, cached per render.
   - **Matches painted with the CSS Custom Highlight API**, zero DOM mutation.
     This is the requirement "search must not corrupt highlights" satisfied by
     construction — a `<mark>` wrap of a hit straddling an existing `mark.hl`
     takes `wrapRange`'s `extractContents` fallback and leaves two elements
     sharing one `data-id`, which nothing repairs. No DOM-wrap fallback on old
     WebKit either: the bar counts and steps unpainted, with a one-time toast.
   - **`#find-bar` docks in `#main-wrap` after `#content-scroll`**, shrinking the
     scroller instead of covering the last line. Absent from the view-mode hide
     list (it belongs to the reading pane) and from the overlay guard
     (`isEditable` already does that work, which is what makes typing a literal
     `/` work with no new code); hidden in print, paint neutralised on paper.
   - Smart case, `.*` regex toggle, `FIND_MAX_HITS = 2000`, 60ms debounce,
     incremental search from the current scroll position via a **binary** search
     over range rects, invalid regex flagged inline rather than toasted.
   - `renderCurrent` calls `invalidateFind()` before the `innerHTML` write and
     re-runs the scan after — the search equivalent of `reanchor`, without which
     a `:w` under an open bar strands dead ranges.
   - Keys `/`, `n`, `Shift+N` through all five places a field name has to exist:
     `config.rs` (fields + defaults), `KEY_ACTIONS`, `wireKeys`, both
     `ui-check.mjs` `KEYMAP` literals plus the third in the nav page and the
     `rows === 21` → `24` assertion, and `fixtures.mjs`. Plus README's keybind
     table and config sample. No Rust *logic* changed; `search.rs`, `nucleo` and
     the `SearchIndex` are untouched and cross-file content search stays unbuilt.

3. **Escape became `:nohlsearch`** on a follow-up request. It already closed an
   open bar; now, with the bar closed and matches still lit (the state `Enter`
   leaves), it puts the highlight out and claims the key, and only falls through
   to leaving view mode when nothing is painted. Non-destructive: the pattern and
   match set survive, so `n`/`N` step on and relight. Tracked by a `findLit` flag
   set in `paintFind` and cleared in `clearFindPaint`, so it is false on a webview
   with no Custom Highlight API and Escape never claims a key for an invisible
   highlight.

### Mistakes & deviations

- **The quick perf tier was unusable, and the first read of it was nearly wrong.**
  It flagged sixteen rows. An A/B against a stashed tree showed the
  `chromium.scroll` rows flagging on unmodified code too, and three consecutive
  runs of the *same* tree read +30%, +376% and +48% on
  `scroll…renderer.raster_ms`. Relaying that as a regression would have been
  false; dismissing all of it would have missed the real one below.

- **`::highlight()` rules cost 27% on every render merely by being declared.**
  `chromium.render.mixed-2m.forced_layout_ms` was the one row that reproduced.
  Isolated by running `scenarios/render.mjs --size 2m` directly, three reps per
  arm: 291±5ms without the change, 369±14ms with it, and 295±3ms with the change
  in place but those two rules deleted — so the cost was theirs alone, with no
  highlight ever registered. Chromium resolves highlight styles across every text
  node once such a rule exists. Fixed by moving them into an empty
  `<style id="find-css">` that `installFindCss()` fills on the first `/`: back to
  294±5ms, and a reader who never searches pays nothing. A harness assertion now
  pins the invariant in both directions so nobody folds the rules back into the
  sheet. Chromium harness, relative signal only.

- The plan's §6 reasoning about `/` versus the pending-mark state machine was
  already stale — `587094e` cut `consumeMarkKey` and `pendingMark` the day before,
  so `/`, `n` and `N` are ordinary single combos with nothing to interact with.
  Noticed while checking, not discovered by breaking something.

- One deviation from the plan, deliberate: the plan wrote the count as a bare
  `3/17`. It now appends `+` when the scan hits `FIND_MAX_HITS`, because a
  silently truncated count is a lie about how many matches there are.

### State

`cargo build` clean. `perf/harness/ui-check.mjs` **70 passed, 0 failed**, up from
64 — sixteen new assertions covering the find bar, the paint, the lazy stylesheet,
`n`/`Shift+N` with the bar closed, invalid regex, and both Escape behaviours.
`cargo run --example config_check` 34/34. A throwaway Node harness drove
`findCompile`/`findScan` straight out of `ui/app.js` (grepped, not copied) for
eleven cases the browser harness cannot reach: smart case both ways, literal
metacharacters, invalid patterns, zero-length matches terminating, `MAX_HITS`, and
Unicode offset soundness (`İ`) — 11/11.

Perf tiers **not run** at the author's request when wrapping up. The targeted
render A/B above stands in for the render path; the scroll rows were not
re-measured and the tier's `chromium.scroll` baseline drift noted in the first
bullet is still outstanding.

Left open, and it is the one thing this design cannot answer from here: **whether
`CSS.highlights` paints in WKWebView at all.** The whole paint path is chosen for
a structural guarantee, and it has only ever been seen in Chromium. Also unchecked
by hand: that `/` reaches the handler on the author's keyboard layout, and that
placing a highlight over a painted match still anchors. `MAX_HITS = 2000` and the
60ms debounce remain guesses.

## 2026-07-27 — view mode showed a blank window, and the overnight batch's last plan landed

Three things the overnight batch left in an odd state: view mode painted an
empty screen, marks worked but felt dead, and jump-back was the one piece of
`file-and-section-links` still unbuilt. All three closed, and the frontend's
correctness harness grew the assertions that would have caught the first one.

### What happened

1. **View mode was rendering the document at width 0** (`ui/index.html`). The
   bug is one CSS line and worth stating precisely, because it is not obvious:
   `body.view-mode #sidebar { display: none }` takes the sidebar out of grid
   *placement*, where `body.nav-collapsed` merely gives it a zero-width track.
   With `grid-template-columns: 0 1fr` still declared, `#main-wrap` therefore
   became the **first** grid item and landed in the `0` track. Measured in
   Chromium before touching anything: `#main-wrap` 0px wide, `#content` 80px.
   Fixed by declaring one track in view mode, with a comment naming the trap and
   the fact that a third child of `#workspace` reopens it.
2. **Marks cut from twenty-six to one** (`ui/app.js`, `src-tauri/src/config.rs`).
   They were never broken — driving the real UI showed `ma` → `]` → `'a`
   restoring file *and* offset exactly. What was broken was that `m` alone did
   nothing observable, so the feature read as dead. The author's call was one
   mark, no letter, which deleted the entire chord state machine that shipped
   with `583a467`: `pendingMark`, `MARK_TIMEOUT_MS`, `armMark`/`clearMark`/
   `consumeMarkKey`, the `e.repeat` guard, the Escape branch, four `clearMark()`
   calls across the overlay and `isEditable` paths, and a `blur` listener. `m`
   and `'` are now ordinary single combos that confirm immediately. That state
   machine was the largest piece of input state in the frontend and it existed
   to serve a second bookmark nobody had asked for.
3. **Jump back / forward built** — the last open item in
   `docs/plans/file-and-section-links.md`, on `Ctrl+[` / `Ctrl+]`. The plan
   recommended `Alt+Left`/`Alt+Right`; the author took its option 3 instead,
   because the brackets echo the bare `]`/`[` file-step one modifier away.
   §3's broad rule (any teleport pushes; scrolling never does) was taken as
   written.
   One design change out of the plan. §4 put a `pushJump()` inside both
   `openFile` and `scrollToFragment`; that double-counts, because a cross-file
   `other.md#section` link *is* `openFile` followed by `scrollToFragment` and
   the second frame would be the new document at offset 0 — a place nobody read,
   and a jump that takes two presses to undo. Cross-file pushes now live in
   `openFile` (a genuine funnel: tree, palette, `]`/`[` and links all pass
   through it) and in-document pushes at their two call sites, where the
   `#anchor` one can also be made conditional on the target actually being
   found. A `restoring` flag keeps a pop from re-pushing its own arrival.
4. **Marks and history share a frame.** `{ path, top }` plus `restoreFrame()`,
   exactly as the plan's §7 said to if both ever got built — shared helper,
   separate storage. A jump to the mark is itself a teleport and goes on the
   history like any other.
5. **`perf/harness/ui-check.mjs` gained a third page** and 15 assertions: view
   mode's measured width, that highlighting still works with the chrome hidden,
   the Esc precedence between annotation and view mode, the mark round-trip, and
   the jump history including the cross-file-section-link case from (3). The
   view-mode checks assert *width* deliberately — every existing check passed
   while the feature painted an empty window.

### Mistakes & deviations

- `/perf-quick` flagged the Chromium scroll rows at +50–60% on
  `main_thread_task_ms` and `composite_ms`, and it **reproduced** on a second
  run. Rather than accept or dismiss it, parked both UI files and re-ran against
  the unmodified tree: the same rows moved by the same amounts. Not this change
  — the baseline is stale for those metrics. Left alone; moving it takes a
  deliberate `perf-deep`.
- The plan was followed except where it was wrong, and the one place it was is
  recorded above and in `ui/app.js` rather than silently worked around. A
  `> Built` note at the top of the plan file says which of its options were
  taken.
- Wrap-up ran with perf testing explicitly skipped at the author's request.

### State

`cargo build` passes. `cargo run --example config_check` 34/34.
`node perf/harness/ui-check.mjs` **56 passed, 0 failed** (41 before this
session). No perf tier run at wrap-up, by request; the quick-tier finding above
was investigated and cleared during the session. `perf/baseline.json` untouched
and arguably stale for the Chromium scroll rows — a `perf-deep` would settle it.

## 2026-07-27 — twelve ideas processed unattended, on a branch, with no perf run

An overnight batch over `ideas/`: one subagent per idea, strictly sequential in a
single checkout, each deciding for itself whether its idea was cheap enough to
build or big enough to only plan. Twelve ideas in, twelve commits out, no
failures. Two deliberate departures from house style, both because nobody was
watching: everything landed on `claude/overnight-ideas-2026-07-27` for review
rather than straight to `main`, and **no perf tier was run at any point**.

### What happened

Nine implemented, three of those splitting off a plan for the risky half; two
implemented-plus-planned-refinements; one plan-only. Ordered as they landed.

1. **`contents-outline-panel` — implemented** (`790a883`). Headings now get
   GitHub-style slug `id`s in `markdown::render_with`, via a new `pub`
   `markdown::Slugger` (repeats get `-1`, `-2`, with the numbered candidate itself
   checked for clashes, so uniqueness is a guarantee rather than a near-certainty).
   On top of that a `#outline-panel` built by walking the rendered DOM — no extra
   IPC — with `toggle_outline` on `Ctrl+I`. Sequenced first on purpose: two other
   ideas needed heading ids. The open question is answered *both* ways — an open
   panel tracks `file-changed`, a closed one just sets a dirty flag.
   It also fixed a latent bug: `interceptLinks` did `querySelector(href)`, and
   `## 1. Intro` slugs to `1-intro` — a valid *id* but an invalid *id selector*,
   so the very first numbered heading would have thrown.
2. **`file-and-section-links` — split** (`7d3224c`). Implemented the containment
   gap the idea file flagged: the file-link handler never confirmed the resolved
   path stayed inside the repo root the way the image handler did. Factoring the
   two together tightened both — `startsWith(repoRoot)` never reaches a path
   separator, so a root of `/w/notes` was also admitting `/w/notes-private/`.
   Cross-file `other.md#heading` now jumps, too. Jump-back went to
   `docs/plans/file-and-section-links.md`: both keys the idea proposed (`Ctrl+O`,
   `Ctrl+I`) were already bound, making the default a muscle-memory call rather
   than something to guess at unattended.
3. **`hide-file-tree-keybind` — implemented** (`1895e69`). `toggle_tree` on
   `Ctrl+B`, rebindable, with `data-tip-key` so the existing buttons show it.
4. **`view-mode-keybind` — implemented** (`308bb46`). `Ctrl+M` (for "minimal";
   `Ctrl+\` was rejected for needing escaping in a TOML basic string). `body.view-mode`
   is purely *additive* — it never writes `nav-collapsed` or a panel's `open`
   class, only out-ranks them — so exiting restores exactly the chrome you had and
   nothing can be stranded. The cost is that CSS source order is now load-bearing,
   commented in place.
5. **`jump-top-bottom-keybind` — split** (`1f724a2`). `Home`/`End` as ordinary
   rebindable actions; `gg`/`G` planned. This is where the batch's most reused
   finding came from: **chord support does not exist anywhere**. `matchCombo` is
   fully stateless and `onRecordKey` captures exactly one keydown, which settles
   the same open question in two idea files.
6. **`next-prev-file-keybind` — implemented** (`3b8be57`). `]`/`[`. The ordering
   is read from the sidebar DOM rather than re-derived in Rust, which is what the
   idea file's "don't invent a second ordering" warning was actually asking for:
   there is now only one ordering in the system, and it follows watcher rebuilds
   and File ▸ Open for free. Wraps at the ends, because stopping is
   indistinguishable from a dead key.
7. **`code-copy` — implemented** (`558feae`). Needed no Rust: `copy_to_clipboard`
   was already registered. The button carries **zero text nodes** — SVGs, wording
   on `aria-label`, confirmation via the toast outside `#content` — so injected
   chrome cannot leak into `getSelection().toString()` and corrupt a highlight
   quote.
8. **`reading-progress-indicator` — split** (`5e2cad1`). Percentage, not heading
   count: heading sizes are wildly uneven, so "section 3 of 12" is confidently
   wrong as a *position*, and structure already has a home in the outline panel.
   Two surfaces split by kind — a `#progress-pct` readout in the titlebar
   (chrome), a 3px `#progress-rail` at the foot of the reading pane (ambient).
   View mode keeps the rail and drops the readout. Built for no perf feedback:
   passive listener, one rAF per frame, metrics cached off a `ResizeObserver`,
   composited `transform` rather than `width`.
9. **`stack-panel-polish` — implemented, both halves** (`e8e7043`). `refreshStack()`
   keyed-reconciles instead of wiping `innerHTML`, with snap-in/snap-out motion
   gated on `prefers-reduced-motion`, view mode, and a closed panel. The hazard
   found on the way is the important part: `send_stack` resolves ids through
   `Store::selected_pairs`, which looks up the **highlight** list rather than the
   stack — so a checkbox outliving its card's ~170ms exit would have sent a pair
   the reader had just removed. Guarded twice (the checkbox is deleted the instant
   a card is marked `.leaving`, and `checkedIds()` filters `.leaving` as well),
   plus a `stackSeq` guard because stable keys let a late `get_stack` reply
   resurrect a gone card in a way the old teardown could not.
10. **`md-to-pdf-export` — implemented, refinements planned** (`3218fdf`). The
    idea's core recommendation held — no PDF crate, no dependency, no CSP change —
    but its trigger was wrong: **`window.print()` is a silent no-op in WKWebView**,
    because WebKit routes it to the UI delegate and wry's `WKUIDelegate` doesn't
    implement `_webView:printFrame:`. Shipped as literally described, the button
    would have done nothing. So it's a six-line `print_document` calling
    `WebviewWindow::print()`. The bulk is `#print-css`, deliberately the *last*
    `<style>` in `<head>` so it beats the runtime-injected theme: it neutralises
    the palette at variable level (the default theme is dark — otherwise the page
    prints grey-on-white) and flattens `#content-scroll` out of its `100vh` column
    so a document scrolled to 60% paginates in full rather than printing its
    middle third. Content-only.
11. **`vim-marks-bookmark-jump` — implemented** (`583a467`). Global marks, not
    per-file: dreamd shows one document out of a whole repo, so the useful move is
    cross-file by definition. The chord plan written in (5) over-estimated the cost
    for *this* shape — the letter is an *argument*, not part of the binding, so
    `set_mark`/`jump_mark` stay ordinary single combos and the rebind UI never
    learns marks exist. That left one risk, the pending-prefix state machine, built
    around a one-sentence invariant: it consumes only key repeats while a leader is
    held and a bare alphanumeric within 1.5s; everything else cancels and falls
    through **unprevented**, so a mark going wrong costs the mark, never the
    keystroke. Jump defaults to `'` rather than the idea file's `` ` ``, which is a
    dead key on several international layouts.
12. **`vim-style-content-search` — plan only** (`2808e8f`). It paints the same DOM
    highlight anchoring reads, spans five files across two languages, and none of
    it is checkable without a webview or a perf tier; there is no useful narrow
    slice, since a bar that finds but doesn't paint is worse than no bar. The plan
    **overturns the idea file's central decision**: raw-source search assumes the
    source is already resident in the frontend, and it never has been (every
    `read_source` is Rust-side, `render_markdown` returns HTML), so it would need
    new IPC — while the offset→DOM mapping the idea calls "the main hidden cost"
    already exists and is already in use (`scanTextNodes`/`nodeIndexAt`, built for
    `applyHighlights`). Searching flattened rendered text is simpler, no slower
    where it counts, and drops both the syntax-noise and `te**s**t` blind spots the
    idea file had conceded. It also recommends the CSS Custom Highlight API over
    `<mark>` wrapping, which has a concrete corruption path: a hit partially
    covering a `mark.hl` takes `wrapRange`'s `extractContents` fallback and leaves
    two marks sharing one `data-id`.

### Mistakes & deviations

- **`cargo build` does not pass on Linux, and never did.** The container came
  without the GTK/WebKit dev libraries (installed via apt), and even then the bin
  target fails with 5 errors on a clean `origin/main`: `dreamd::menu` and
  `open_target` are `#[cfg(target_os = "macos")]` but referenced un-gated at
  `main.rs:863-865`. Pre-existing and not touched — changing the platform gating
  was well outside what an unattended batch should decide. The gate became
  two-part instead: `cargo build --lib` must pass outright, and the bin must
  produce *exactly* those 5 errors and no more.
- **That gate was initially mis-described as weaker than it is.** The first read
  was that `main.rs` gets no compile check here. It was checked properly during
  idea 10 and that is wrong: injecting a bogus method call took the count 5 → 7
  and the gate failed, so `main.rs` **is** genuinely type-checked — the 5 errors
  are name-resolution failures that don't stop the rest of the file being checked.
  Every Rust change in this batch therefore did get a real compile check.
- **No perf tier was run, by instruction** — see State.
- **Two ideas were deliberately out of scope**: `file-import-to-markdown.md`
  (blocked) and `live-file-tree-sync.md` (closed 2026-07-26; live sync works in
  day-to-day use, and the "don't trust it, verify it" section still in that file
  is superseded). Neither was implemented, planned, or otherwise touched.
- No idea failed, and no agent left the tree broken, so the fallback path
  (`git reset --hard` plus a failure log) was never used.
- One consequence of the batch shape worth naming: twelve agents each judged their
  own risk, so "implemented" here means *one* agent was confident, not that a
  second reviewed it.

### State

- **Branch `claude/overnight-ideas-2026-07-27`, not `main`** — 12 idea commits
  plus this one, opened as a PR for review.
- **Perf was intentionally not run.** No `perf-quick`, `perf-pass`, `perf-deep`,
  `perf/run.sh` or `cargo bench` at any point, because the harness is not
  confirmed working on Linux or off the author's machine. `perf/baseline.json` is
  untouched. **This is left entirely for the author to check locally** — and
  several changes are exactly the kind that want it: the scroll-driven progress
  indicator, the stack panel's new reconciliation and animation, `decorate_code`
  (which adds a new `d:decorate_code` perf phase with no baseline entry), and the
  heading-slug pass added to every render.
- **Build**: `cargo build --lib` clean; bin at exactly the 5 pre-existing
  macOS-gating errors after every commit. `cargo run --example config_check` 34/34
  after each `Keymap` widening; `locate_check` 611 fixtures, 0 wrong/moved/
  unresolved. `node --check` on every touched JS file.
- **Nothing has been seen running.** No macOS, no Tauri window, and
  `perf/harness/ui-check.mjs` could not run either — Playwright's Chromium
  download is 403'd by this container's proxy. Its keymap fixtures and its
  `rows === N` assertion were bumped by hand across five commits (now 19) and are
  **unverified**; that is the single most likely thing to be wrong. No CSS in this
  batch has ever been rendered.
- **Six new keybinds**: `Ctrl+I` outline, `Ctrl+B` tree, `Ctrl+M` view mode,
  `Home`/`End` document ends, `]`/`[` next/prev file, `m`/`'` marks.
- **One deliberate behaviour change**: stack checkbox ticks now persist across
  refreshes, a consequence of stable keys. One-line revert noted in the log.
- Five plans are parked in `docs/plans/` for later: jump-back, `gg`/`G`,
  heading-aware progress, print refinements, and content search.

## 2026-07-26 — signing on, v0.1.0 shipped, and a tap job that had never worked

Same day as the entry below, and the reversal of it: a Developer ID certificate now
exists, so signing went on and dreamd 0.1.0 shipped signed, notarized and installable
with `brew install --cask bongofongo/tap/dreamd`. Getting there meant catching an
unsigned draft that was one click from being published, and finding that the `tap`
job had been incapable of succeeding since the day it was written.

### What happened

1. **`packaging/SIGNING.md`** — a twelve-step runbook for turning signing on, written
   before touching anything: export the `.p12`, base64 it without newlines, mint an
   app-specific password, **validate locally before uploading**, upload from the same
   shell, add the tap token, flip the switches, rehearse with `workflow_dispatch`,
   verify the artifact under quarantine, tag, update the site, clean up. It carries a
   failure table mapping every `check-signing.sh` error to its fix, plus rotation and
   back-out sections. The ordering constraint that matters is step 5 before step 6:
   `check-signing.sh` costs ten seconds, the matrix costs twenty minutes.
2. **The secrets went in and the switches flipped** — six `APPLE_*` secrets plus
   `TAP_GITHUB_TOKEN`, `PUBLISH_CASK=true`, `NO_SIGN` deleted. The `workflow_dispatch`
   rehearsal passed: `verify` green, both arches signed and notarized, `release` and
   `tap` correctly skipped.
3. **Artifact verification, both arches.** Recorded vs actual sha256, full authority
   chain to Apple Root CA, `flags=0x10000(runtime)`, secure timestamp, stapled ticket,
   and the one that matters — `xattr -w com.apple.quarantine` then `spctl`, which
   returned `accepted, source=Notarized Developer ID`. The quarantined binary also
   ran: `dreamd --version` and `dreamd theme list` both work from a quarantined
   bundle, which is the cask's `binary` stanza proven end to end.
4. **The existing `v0.1.0` draft was unsigned.** Built at `a972d9b`, one commit
   before signing went on: `Signature=adhoc`, `TeamIdentifier=not set`, no stapled
   ticket, `spctl` refusing it. With `PUBLISH_CASK` now true, publishing that draft
   would have pushed a cask pointing at ad-hoc artifacts — "dreamd is damaged" for
   every Homebrew user. Deleted the draft and the tag, re-tagged `v0.1.0` at the
   signed commit. The version number was unspent, so nothing user-visible was
   rewritten.
5. **Three faults in the `tap` job, none of which had ever run** (`bc4e2a8`,
   `bd157c4`):
   - `grep -q '@@'` matched `cask.rb.tmpl`'s own header comment — *"substitutes the
     three @@ tokens below"*. `sed` never touches a comment, so the guard fired on
     every render. It had a 100% failure rate from the day it was written; v0.1.0 is
     simply the first release that got far enough to reach it.
   - `brew audit <path>` is disabled from Homebrew 6 ("Use `brew audit [name ...]`
     instead"), and auditing by name needs the cask reachable as a tap — so the job
     now symlinks the checkout into `Library/Taps` first.
   - `brew audit --cask` is macOS-only and the job ran on `ubuntu-latest`. Moved to
     `macos-latest`.
   Separately, `brew style --fix` rewrites `on_arm`/`on_intel` into
   `sha256 arm:, intel:` and `">= :catalina"` into `:catalina`, so the committed cask
   could never have matched its template. `cask.rb.tmpl` is now written in the shape
   brew wants and renders byte-identical to what gets committed — verified against
   Homebrew 6.0.12: `no offenses detected`, `audit --online` exit 0.
6. **`git push -u origin HEAD` in the tap job** (`ba634a9`) — the tap repo was empty,
   so the first bump would have committed onto an unborn branch and aborted on a bare
   push, at the very end, after publishing. Seeded the tap with a README as well, so
   the already-tagged workflow could not hit it either.
7. **The site now leads with Homebrew.** `BREW_CASK` in `consts.ts`, the brew line
   first, curl demoted to a `without Homebrew` fallback under a new quieter `.alt`
   eyebrow, and the download button finally pointed at `RELEASES_URL`. The
   "Homebrew follows once there is a signature to check" sentence is gone. Built
   clean: HTML 4.92 KB gzip, CSS 2.03 KB, JS still 0.

### Mistakes & deviations

- **The release.yml header contradicted itself.** `NO_SIGN` was deleted when signing
  went on, but the paragraph above it still explained curl-only distribution, with
  `# SIGNING IS ON` appended to the end. Rewritten in `f57273b` to say what the
  *absence* of the key means, and not to reintroduce it to get past a red build.
- **Assumed a fix on `main` would reach the failing job.** It does not: a `release`
  event runs the workflow *from the tag*, and `v0.1.0` points at `f57273b`, which
  predates all three tap fixes. Re-running would have replayed every bug. The cask
  for 0.1.0 was therefore rendered and committed by hand after local `brew style` and
  `brew audit --online` both passed; the fixed job gets its first real exercise on the
  next release.
- **Nearly recommended re-running the release workflow afterwards.** It would have
  rebuilt with a fresh notarization ticket and timestamp, changing the zip bytes and
  therefore the sha256, while the published cask still carried the old ones — turning
  a working install into a checksum mismatch for everyone. Stopped at the question.
- The environment's classifier blocked writing to the tap repo, so both tap commits
  were handed over as commands rather than run here.

### State

`v0.1.0` published, both arches notarized and stapled, asset sha `2dafe5c8…` matching
the cask in `bongofongo/homebrew-tap`. `brew install --cask bongofongo/tap/dreamd`
confirmed working by the user. No Rust changed this session, so no `cargo build` gate
and no perf tier — the commits are CI, packaging, docs and site only.

Open: the site build is verified but **not deployed** (`npm run deploy` is manual and
publishes live). The `macos-latest` + symlink-audit path in the `tap` job is verified
locally against Homebrew 6.0.12 but has not yet run in CI; the next release is its
first real test.

## 2026-07-26 — the first release failed on signing; ship curl-only instead

The first tagged release died in the bundler with `SecKeychainItemImport: One or more
parameters passed to a function were not valid`. Diagnosed it, built a preflight so it
can never cost twenty minutes again, and then — on learning the account has no
Developer ID certificate — narrowed distribution to the one channel that does not need
one. Releases are now unsigned and curl-only, by decision rather than by omission.

### What happened

1. **The error, decoded.** The Tauri bundler base64-decodes `APPLE_CERTIFICATE` to a
   temp file and runs `security import`. That message is `errSecParam`: the decoded
   bytes are not a PKCS#12 bundle. It names none of the six `APPLE_*` secrets, and it
   arrives *after* a full Rust build, on both matrix legs at once.
2. **`packaging/check-signing.sh`** — a preflight that reads the same environment
   `build.sh` does and says which secret is wrong. It separates the near-misses that
   all produce the same upstream error: a `.cer` (public half, no private key) or a
   PEM instead of a `.p12`, base64 with newlines (the bundler's decoder is strict),
   a blank export password, a wrong export password, an `Apple Development`
   certificate where Developer ID is required, `APPLE_SIGNING_IDENTITY` not matching
   the certificate's CN, a team-id mismatch, an expired cert, and an account password
   where an app-specific one belongs. Wired into the existing `verify` job, so it gates
   the matrix for ~10s. Exercised against synthetic p12/cer fixtures — every branch
   above was driven and produces the right message.
3. **It immediately earned its keep.** The user's export was the `Apple Development`
   identity; the script named it rather than letting the matrix rediscover it.
4. **No Developer ID certificate exists** and enrolment is a $99/yr decision, so we
   priced what signing actually buys. `com.apple.quarantine` is written by the
   *downloading application*, not by the OS: curl never writes it, browsers and
   `brew install --cask` both do. So `packaging/install.sh` can install an unsigned
   app that Gatekeeper never inspects, while the other two channels hand the user
   "dreamd is damaged".
5. **Decision: curl-only for now.** `NO_SIGN: "1"` at workflow level in
   `release.yml` is the entire switch — `check-signing.sh` stands down, `build.sh`
   passes `--no-sign` and skips the codesign/spctl/stapler asserts. The `tap` job is
   additionally gated on a `PUBLISH_CASK` repo variable that is not set. Zip artifacts
   still ship: they are the transport `install.sh` fetches, not a browser download.
6. **Site copy corrected.** `website/src/pages/index.astro` claimed "Signed and
   notarised" and led with the Homebrew line — both would have been false the moment a
   tag landed. Now one `curl` command, a note explaining why not to fetch the zip by
   hand, and the download button removed. `RELEASES_URL` stays exported and unused for
   when signing returns.
7. Both `CLAUDE.md`s record the three steps that undo all of this: certificate, six
   secrets, delete the `NO_SIGN` key, `gh variable set PUBLISH_CASK --body true`.

### Mistakes & deviations

- **Prompted for a passphrase mid-run.** The preflight passed the p12 password via
  `-passin env:`, and openssl writes its prompt to `/dev/tty` — so a source it cannot
  read becomes an interactive hang or a prompt scrolling past in a CI log, not an
  error. Switched to a `0600` temp file plus `</dev/null` on both openssl calls: an
  unreadable password now fails immediately with a message. Re-tested both ways.
- **A test that proved nothing.** The "wrapped base64" case appeared to pass the
  whitespace check. It wasn't a script bug — macOS `base64` emits a single line, so
  the fixture had no newlines to catch. Re-ran through `fold -w 76` and the check
  fired correctly. Worth remembering: GNU `base64` wraps, BSD does not.
- No plan changed direction otherwise; the signing diagnosis was correct first time.

### State

Docs, CI config, one shell script, and site copy — nothing under `src-tauri/` or
`ui/`, so no `cargo build` gate and no perf tier was run; the binary is untouched.
`npm run build` in `website/` is clean and `RELEASES_URL`/`btn-quiet` have no dangling
references. `check-signing.sh` was verified against generated certificates covering
every failure branch, and `NO_SIGN=1` correctly short-circuits it.

Open, and deliberately so: releases are unsigned, Homebrew is parked, and the site is
**not deployed** — `npm run deploy` publishes live and was not run. The next tag will
produce an unsigned draft release; `cargo tauri build --no-sign` should be run once on
real hardware first, since the GUI cannot be launched from this environment.

## 2026-07-25 — a shippable macOS app: silent launch, half the binary, a release pipeline

Set out to make dreamd installable on macOS — a downloadable `.app`, the CLI on
`PATH`, a minimal binary, and CI that a Linux target could later join. Got there, with
one correctness bug found on the way that would have made the first download unusable.

### What happened

Two Explore agents mapped the build surface and a Plan agent verified the design
against the actual crate sources (tauri 2.11.5, tauri-codegen 2.6.3, tao 0.35.3,
syntect 5.3.0). That verification pass corrected the plan three times before any code
was written — see *Mistakes*.

1. **A double-clicked `.app` would have hung the machine.** LaunchServices gives a
   Finder launch cwd `/`. `resolve_repo_root` walked up looking for `.git`, found
   none, and returned `/` — which `main.rs` then handed to `Catalog::build`
   *synchronously, before the Tauri builder exists*, as an `ignore::WalkBuilder` with
   `hidden(false)` and no depth limit. The window could not appear until the whole
   filesystem had been walked. This is a packaging blocker, not a polish item, so it
   was fixed first.
2. **Silent launch.** `resolve_repo_root_found` reports whether a `.git` was actually
   found, and `resolve_target` returns it as a third `has_repo` field — an explicit
   path is still always honoured, because someone who types `dreamd ~/notes` means it.
   With no repo: nothing walks, the watcher is not armed (it would `watch("/")`
   recursively), and the window — now `visible: false` in `tauri.conf.json` — is never
   shown. `AppState` gained `has_repo: AtomicBool` beside the existing `appearance`
   atomic; `repo_root` became an `RwLock<PathBuf>` because File ▸ Open moves it.
   Showing the window from `.setup()` *after* `set_background_color` also closes the
   white-flash gap for every normal launch.
3. **A native menubar, the first `.menu()` call in the codebase.** Tauri's default
   macOS File menu is only "Close Window", and there is no way to add one item —
   supplying a menu replaces the whole bar, so `src-tauri/src/menu.rs` rebuilds it
   from `PredefinedMenuItem`s with Open Folder… (`⌘O`) and Open File… (`⌘⇧O`).
   **`Ctrl+O` was left alone as `toggle_stack`**: `matchCombo` in `ui/app.js` requires
   exact modifier equality including `metaKey`, so a `⌘` chord can never reach a
   `Ctrl` binding. No keymap change, no migration, no README churn.
4. **`rfd` called straight from Rust** rather than `tauri-plugin-dialog` — the trigger
   is a menu event already on the Rust side, so the plugin would have cost a
   registration and an ACL entry to reach the same NSOpenPanel. `Catalog::settle_empty`
   opens the readiness gate without walking, because `wait_tree` would otherwise block
   forever and the frontend's boot would hang instead of reaching its empty state.
   `paintTree` previously rendered an empty sidebar as *literally nothing*; it now says
   which kind of empty it is.
5. **`watcher::spawn` gained a cancel flag.** The thread owns its watcher for its
   lifetime, so a second File ▸ Open would have left one watching the old root and
   emitting `file-changed` for files the window no longer shows. The pump's outer
   `recv` became `recv_timeout(500ms)` so an idle watcher still notices retirement.
6. **43% of the binary was the app icon.** tauri-codegen decodes the first `.png` in
   `bundle.icon` to raw RGBA and `include_bytes!`s it — `icon.png` at 1024² is exactly
   4,194,304 bytes. On macOS it is never read at all: `set_window_icon` is a documented
   no-op in `tao/src/platform_impl/macos/window.rs`. Reordering the array so
   `128x128.png` comes first (65,536 bytes — 32² would save another 61 KB but the
   Linux WM genuinely uses this icon) plus `arboard` without `image-data`, explicit
   syntect features, and `tauri` without `dynamic-acl`/`common-controls-v6` took the
   release binary **9,782,112 → 5,571,648 bytes**. `__const` went 5,411,972 → 1,298,860.
7. **The release pipeline is `packaging/build.sh`; the workflow only wraps it.** Every
   platform-specific decision is derived from the target triple *inside the script*,
   never encoded in the CI matrix, so a Linux target is one matrix entry plus one
   `case` arm. Tag `v*` → both arches → draft release; **publishing** is what bumps the
   Homebrew cask, so nothing reaches users until a human has double-clicked the build.
8. **`ditto`, not `tar`.** Part of a `.app`'s signature lives in extended attributes;
   tar drops them and the extract fails Gatekeeper in a way that looks exactly like a
   notarization problem. Artifacts are `.zip`, and `install.sh` unpacks with `ditto -x`.
9. **`trash` pinned to `DeleteMethod::NsFileManager`.** Its macOS default `osascript`s
   Finder, which under the hardened runtime would need `NSAppleEventsUsageDescription`
   plus an Apple Events entitlement and a permission prompt on first delete. Switching
   backends meant the app ships with **no entitlements file at all**. Cost, accepted
   deliberately: Trash's "Put Back" no longer appears.

### Mistakes & deviations

1. **The plan had `dump-create` down as droppable from syntect.** It is not:
   `html → parsing → dump-create`. Only `plist-load` and `yaml-load` can go. Caught by
   the Plan agent reading syntect's own manifest rather than trusting the feature names.
2. **The plan said `tar` with `COPYFILE_DISABLE`.** Wrong for a signed bundle; see 8.
3. **The plan proposed version-pinned download URLs on the website.** The site deploys
   by a manual `npm run deploy` that is independent of the release workflow, so a pinned
   href would 404 for everyone between "tag pushed" and "site deployed". Changed to
   `/releases/latest`, which needs no redeploy ever.
4. **A `"//visible"` comment key in `tauri.conf.json` was a hard build error** — the
   window config is `deny_unknown_fields`. The explanation moved into the Rust.
5. **The dmg could not be built and was dropped.** Tauri's dmg bundler `osascript`s
   Finder to pose the window and dies with `AppleEvent timed out (-1712)`; it produces
   a valid image with `--skip-jenkins`, but that flag is not reachable through the
   bundler and the failure is a known coin-flip on CI runners. Since the cask and the
   installer both consume the `.zip`, shipping only the `.zip` was the user's call.
6. **The first perf pass reported `bench.render/code/128k` +33.6% and I nearly
   believed it.** `spread_ms` had gone 261 → 6448 — the machine was not quiet. A direct
   A/B put the same bench at +2.8%, and Criterion's own delta against the contaminated
   run read −14.9%. The clean re-run dropped `bench.render/*` out of the moved list
   entirely. **Diffing against the stale baseline was equally misleading in the other
   direction** — it shows a wall of phantom improvements from an earlier session.
7. **The website's own verification script was wrong before the site was.** Two
   `.brand` assertions failed; both were my test — `html` has `scroll-behavior: smooth`
   so `scrollTo` animates, and `.brand` has a 0.35 s opacity transition that a 200 ms
   wait samples mid-way.
8. **Backgrounding the deep run through `tail -70` lost the summary's flagged row.**
   Reconstructed by diffing the results file against the baseline in git rather than
   guessing at it.

### State

`cargo build` green. `config_check` 34/34, `theme_check` 20/20 (10 families × 2 modes),
`locate_check` 0 failures over 611 fixtures, `ui-check` 41/41. A purpose-written
Chromium script checked the site at four viewports: sticky landing, no overflow, no
console errors, 0 JS files, reduced-motion clean.

The launch fix is verified by measurement, not inspection: from `/`, startup now exits
at 3.9 ms emitting only `process_start → target_resolved → config_loaded`, with no
`walk_done`/`index_built`/`tree_built`. From inside a repo all three still fire at ~43 ms.

**`perf/baseline.json` was updated by a deliberate `perf-deep --update-baseline`**, at
the user's request, in this commit because this is the change that justifies it:
`release_binary_bytes` 6,396,272 → 5,571,648, and `real.loop.release-h100.events_per_save`
1.583 → 1.083 (that one is an earlier session's debounce fix finally being captured, not
this session's work). No regression: the 5 rows still reading slower are a *pixel height*
(`documentPx`, identical to the pre-change run) and sub-millisecond metrics that jittered
±40% between two runs of my own.

From the release profile, for whoever optimises next: pre-window startup is **97% the
repo walk** (57.7 ms of 59.7 ms on a 5000-file repo), and the save loop is dominated by
**reanchoring, not rendering** (`ipc_reanchor` 904 ms p50 vs `ipc_render_markdown` 160 ms
p50, with 100 highlights).

Open, and deliberately not fixed here:

- **The site's wordmark does not return you to the top.** `.brand` links to `#top` =
  `.landing`, which is `position: sticky`, so anchor-scrolling to it only applies
  `scroll-padding-top` and moves up exactly 74 px from any offset (measured 900→826,
  1800→1726, 2600→2526). `website/CLAUDE.md` claimed this worked; the doc is corrected,
  the bug is not.
- Nothing has been built with a **Developer ID Application** cert yet — the local
  signed build used an Apple Development cert, which cannot be notarized and is not
  distributable. The first real artifact needs that cert plus a run of
  `packaging/build.sh`.
- `packaging/install.sh` and the Homebrew tap are untested end-to-end, because neither
  can be until a release exists.

## 2026-07-25 — ten theme families, two appearances each, a literary default

Executed `docs/todo.md` sections 2–4 in one pass: a reading/literary theme pack, a
dark/light toggle, and real design attention on the default. Section 1 (the settings
panel) turned out to have shipped two sessions ago, so the whole file was rewritten
as follow-ups rather than ticked off.

### What happened

**The shape, settled in plan mode.** Two Explore agents mapped the theming surface and
a Plan agent pressure-tested the plumbing; it found three things the plan had wrong,
all before any code (see *Mistakes*).

1. **One file per family, two mode blocks.** `ui/themes/*.css` went from ten flat
   palettes to ten families: a bare `:root` of shared type metrics plus
   `:root[data-mode="light"]` and `:root[data-mode="dark"]` colour blocks. The
   frontend sets `data-mode` on `<html>`, so switching appearance is one attribute —
   no IPC, no re-inject, no visible swap. Same file count as before.
2. **`theme::mode_slice`** is what makes the two values Rust parses by hand
   (`--bg`, `--syntax-theme`) mode-aware. A brace-depth scanner that skips strings,
   descends into non-mode blocks (so an `@media` wrapper needs no special case), and
   drops the other appearance's blocks **and moves yours to the end**. The move is
   load-bearing: `custom_property` is a last-wins *textual* scan, but CSS ranks
   `:root[data-mode=…]` (0,1,1) above `:root` (0,0,1) whatever the source order, so
   dropping alone would have disagreed with the webview — visible only as a window
   frame in the wrong colour.
3. **A stylesheet with no `[data-mode]` block is returned unchanged.** That is the
   backwards-compatibility guarantee, and it is why the test is "has no mode blocks"
   rather than "keep the `:root` blocks" — a hand-written `theme_css` may declare
   `--bg` on `body`, and a `:root` filter would have thrown it away. Step 1 was
   committed-shaped on its own: `theme_check` staying green against the *still-flat*
   palettes, in both schemes, was the proof.
4. **`Config.mode: Option<Mode>`**, `system` by default. An `Option` rather than a
   plain field with a `System` default because the two are not the same thing — see
   *Mistakes* 4.
5. **`System` is resolved in `.setup()`**, from `Window::theme()`. Config-declared
   windows are created inside Tauri's own setup, *before* the user hook, so a window
   exists there; the call resolves inline on the main thread. `theme_bg` at
   `main.rs:452` is gone, folded into the hook that already consumed it.
   `AppState.appearance` is an `AtomicU8` so `syntax_theme()` — which runs per render
   holding the config lock — never has to think about lock order. A forced mode also
   pins the native appearance via `win.set_theme`, which on macOS is
   `NSApplication.appearance`: app-wide, not per-window, and while pinned
   `Window::theme()` reports the pin rather than the OS.
6. **The frontend resolves it a second time**, deliberately: `matchMedia` at module
   scope in `app.js`, before anything paints, plus a `tauri://theme-changed` listener
   as the runtime-guaranteed path. `get_theme_css` became `get_theme` returning
   `{css, mode, scheme}` — one command changing shape rather than a second call,
   because boot IPC count is on the critical path. `set_appearance` **returns** the
   new view and is awaited: fire-and-forget leaves a one-render window where
   `render_markdown` uses the old syntect theme against the new palette.
7. **`loadTheme` stopped re-parsing `--syntax-theme`** and reads
   `getComputedStyle(document.documentElement)` instead. The engine has already done
   the cascade, specificity, quotes and `@media`, and a value it resolved cannot drift
   from `theme::custom_property` the way a second implementation would. `readCssVar`
   survives only for CSS *not* applied to the document — swatches and the Custom tab —
   and gained a JS `modeSlice` mirroring the Rust one.
8. **`renderCurrent`'s overloaded flag was split.** `preserveScroll` also decided
   whether to re-anchor, which is right for a file that changed on disk and wrong for
   an appearance switch: nothing moved, so re-anchoring every highlight is work for no
   result, and on a sunset auto-switch with a large document, a visible one. Now
   `{preserveScroll, reanchor}`.
9. **Ten families.** Merges preserving colours exactly: `gruvbox`, `catppuccin`,
   `high-contrast` (the one family whose two halves already agreed on type metrics, so
   the shared block was free). New siblings authored: canonical Solarized dark, Nord
   Snow Storm light, Tokyo Night Day light. Three new literary families —
   `manuscript` (Iowan Old Style, sepia / vellum by candle), `letterpress` (Charter,
   justified with hyphenation, one press-red), `athenaeum` (Hoefler Text, brass on
   green-black). All macOS-shipped faces, ending in a generic; no bundled font files,
   no CSP change.
10. **`dreamd` became a literary default** — `ui-serif`/New York at 17px/1.75/700px,
    lilac-cast paper by day, indigo by night. Deliberately not sepia: that is
    `manuscript`, and the default should read as itself.
11. **Optional shape variables** consumed by `theme.css` with fallbacks, so no
    existing or user palette is required to declare them and `REQUIRED` did not grow:
    `--font-heading`, `--heading-weight`, `--heading-rule`, `--letter-spacing`,
    `--para-spacing`, `--text-align`, `--hyphens`, `--code-bg`, `--hl-text`,
    `--stale-text`.
12. **`ALIASES`** maps the seven pre-family names onto family + scheme. Three rules,
    each guarding a specific failure: `dreamd`/`nord`/`tokyo-night` are *absent*
    (they are family names now, and aliasing them would pin dark for every existing
    config); the alias step is the **last** resort in `palette()`, after a user file,
    so `dreamd theme new gruvbox-dark` still wins; and the implied appearance loses to
    an explicit `mode`. `dreamd theme set <old-name>` migrates the config in one
    atomic `patch_global`, and `resolve` reports the *canonical* name so a config
    still saying `catppuccin-latte` marks the `catppuccin` card active.

**Contrast was computed, not eyeballed.** The four families dreamd authors clear
4.5:1 on `--text`, `--muted`, `--link` and `--accent` against their own `--bg` in both
appearances; `--muted` was the binding constraint and pushed three of them darker. The
six *named* palettes keep their published colours where they fall short — Nord light's
`--link` is 3.5:1 and canonical Solarized dark is 4.75:1 on body text by design.
Quietly darkening Nord would make it not-Nord, and that is said in the file.

**Two pre-existing bugs fixed on the way.** `mark.hl` was `background: var(--hl);
color: inherit` — light grey on pale yellow on every dark theme since v1, while the
titlebar button two hundred lines up already hardcoded `#1a1a1a`. And Tokyo Night
Day's `--hl` was `#8c6c3e`, a dark brown: fine as a syntax colour, wrong as a
highlighter, where the mark is a background the text has to survive.

### Mistakes & deviations

1. **The pre-paint bootstrap could not have been an inline `<script>`.** The plan put
   it in `<head>`; `tauri.conf.json` sets `script-src 'self'` with no
   `'unsafe-inline'`, so it would have been blocked *silently* and `data-mode` would
   simply never have been set before paint. Flagged by the planning agent, verified
   against the config, moved to module scope in `app.js`.
2. **Dropping the other mode's block is not enough** — see *What happened* 2. Also
   the planning agent's catch.
3. **`theme::custom_property` and `readCssVar` were documented as mirrors and were
   not.** Rust terminated a value on `;` only; JS on `;` or `}`. Inert while a palette
   was one block; once mode blocks are appended after the shared block, a
   semicolon-less final declaration swallows them. Fixed in the same edit.
4. **`Config.mode` shipped as a plain field first, and `config_check` caught it.**
   With `Mode::System` as the default, "never set" and "explicitly system" are the
   same value, so a legacy alias's implied appearance would have beaten the user
   choosing *System* in the panel — making the toggle a no-op for anyone still on
   `gruvbox-dark`. That is the headline feature of this work. Changed to `Option`.
5. **Cached block ranges in the Custom tab's var editor.** Any edit that changes the
   string's length — `#fff` to `#ffffff`, or an insertion — invalidates every range
   after it. Rows now carry the block *key* and ranges resolve at write time. Caught
   while writing it, not by a harness.
6. **Dedup order in `paletteVars` was backwards.** Iterating shared-first attributed a
   variable declared in both blocks to `shared`, sending the edit somewhere the
   cascade ignores.
7. **The `<pre>` background strip cost 11% of `render/prose` and was reverted.** The
   plan had `markdown.rs` strip the inline `background-color` syntect writes, so
   `--code-bg` could own the slab. `perf-pass` flagged `render/prose` +11% — on a
   corpus document with *zero* code fences, so the new function never runs. A worktree
   at HEAD confirmed it (904 µs vs 1.02 ms at 512k), and reverting only `markdown.rs`
   restored it exactly: pure codegen layout from touching the module. Replaced with
   `background: var(--code-bg, var(--btn-bg)) !important` in `theme.css` — `!important`
   because syntect writes it as an *inline* style, which no ordinary rule outranks.
   Verified in Chromium that the rule wins and token span colours survive. Render path
   now untouched by this session.
8. **Nearly reported the stale baseline's phantom wins again.** The first `perf-quick`
   showed render down 75–78%; `perf/baseline.json` still predates `56143c6`. Diffed
   against the last real `pass-*.json` instead, which is also what exposed 7 as real.

### State

`cargo build` clean. `theme_check` 10 families × 2 modes × 20 vars + 7 aliases, 0
failed — it now asserts through `theme::custom_property` rather than a substring test
(which passed the light pass for a variable only the dark block declared), that every
family declares both blocks with differing `--bg` *and* differing `--syntax-theme`,
that every alias resolves, and that a flat pre-family palette reads identically in
both schemes. `config_check` 34/34. `ui-check` 41/41 — pinned to
`colorScheme: "dark"`, since Chromium defaults to light and `app.js` now reads that
before first paint. `locate_check` unchanged (0 wrong with context).

`perf-pass` after the revert: 195 metrics, **0 regressed**, 1 slower
(`chromium.…highlightMode.renderer.composite_ms`, 2.74 → 3.31 ms — sub-4ms,
Chromium-relative, inside that source's 27% noise floor). No Rust bench flagged. The
58 improved rows are the stale baseline, not this work.

The one honest oddity: `real.loop.debug-h10.repaints` came back 2 in both runs against
4–5 historically, with `events_per_save` below 1 (the watcher coalescing saves). With
n=2 the p50/p95 either side of it are close to meaningless and I could not attribute
the drop to anything in this diff. Worth watching rather than explaining away.

Deliberately measured and left alone: `--content-width` 820 → 700px makes the same 2MB
document ~19% taller (`documentPx` +19%), which is why scroll raster falls 24–28% and
composite rises under a millisecond. Consequence of the narrower reading measure, not
a regression.

Open, and now the top of `docs/todo.md`: **nobody has seen any of this in WKWebView.**
`cargo tauri` is not installed here, so ten themes — including three whose whole point
is a serif face — were authored and checked entirely through Chromium and the CSS. The
`system` mode also rests on WKWebView's `prefers-color-scheme` tracking the effective
NSApp appearance inside a Tauri window, which Chromium cannot confirm; the
`tauri://theme-changed` listener is the fallback if it turns out not to.

## 2026-07-25 — Apache 2.0, and an app icon that isn't a blue square

Short session: give the project a real licence, say so on the public page, and
replace the placeholder Tauri icon with the site's mark. Both landed; the site
was rebuilt, driven in Chromium, and deployed.

### What happened

1. **The licence.** The repo had no `LICENSE` and no `license` field, which is
   legally all-rights-reserved — `website/CLAUDE.md` invariant 6 existed precisely
   to stop the site claiming otherwise. Now: `LICENSE` at the repo root (Apache
   2.0 text, `Copyright 2026 Oliver Fong`), `license = "Apache-2.0"` in
   `src-tauri/Cargo.toml`, a Licence section in `README.md`.

2. **The site.** `LICENSE_NAME` / `LICENSE_URL` in `website/src/consts.ts` are the
   single place the licence is named — same rule as `REPO_URL`. Footer nav gets an
   "Apache 2.0" link; the "get it" section gets one line under *Source on GitHub →*
   reading "Open source under the Apache 2.0 licence". Invariant 6 in
   `website/CLAUDE.md` was rewritten from "never claim a licence" to "the licence is
   Apache 2.0, and copy must match the repo" — the point of the rule was never
   silence, it was not outrunning the repo.

3. **The icon.** `src-tauri/icons/icon.png` was a Tauri placeholder — a blue square
   on dark grey. It is now a 1024×1024 rasterization of `website/public/favicon.svg`
   (dark squircle, `--hl` yellow bar, grey and blue lines), transparent corners. One
   mark for the app and the browser tab.

   No `rsvg-convert` or ImageMagick on this machine, so the render went through the
   Playwright Chromium already installed for the perf harness — the SVG is vector, so
   the output is exact, not a proxy. `src-tauri/icons/README.md` records that the SVG
   is the source of truth and how to re-render.

4. **Deployed.** `npm run deploy` from `website/`, publishing to the live zone at
   the user's request.

### Mistakes & deviations

- First render attempt ran the script from the scratchpad with `cwd` set to
  `perf/harness` — Node resolves `playwright` from the *script's* location, not the
  cwd, so it failed `ERR_MODULE_NOT_FOUND`. Copied the script into `perf/harness/`,
  ran it, deleted it.
- The Apache text was taken from a vendored `LICENSE-APACHE` in the cargo registry
  (`memmap2`) rather than fetched; its appendix copyright line was replaced. That
  copy did not survive — see the push below.
- The push was rejected: PR #2 ("Create LICENSE") had landed the same licence
  upstream while this session ran. Merged rather than rebased — the worktree is dirty
  with another session's work and rebase refuses that — and resolved the add/add
  conflict in favour of GitHub's canonical (indented) Apache text, with the appendix
  copyright filled in as `Copyright 2026 Oliver Fong`.
- `README.md` also carries another session's in-flight theme-family rewrite. Only
  the Licence hunk was staged, via a filtered patch — nothing of theirs was
  committed.

### State

`cargo build` passes. No perf tier run and none warranted: the only `src-tauri/`
changes are a Cargo manifest key and a PNG, no code. Site verified in Chromium
against the built output — both licence links resolve to the repo `LICENSE`, no
console errors, `.landing` still sticky at `top: 0`, no horizontal overflow at 375
or 1440, HTML 4.43 KB gzip against a 15 KB budget, still zero JS.

Open: `bundle.active` is `false`, so `icon.png` is the entire icon set — packaging
later needs `.icns`/`.ico` from `cargo tauri icon`, and `tauri-cli` is not installed
here. The dock icon could not be eyeballed; the GUI can't be driven in this
environment.

## 2026-07-25 — deferring the repo walk on a single-file launch

Executed `perf-plan.md`, a handoff plan written in a Linux container that could
neither build the project nor run any perf tier. The plan held up: `dreamd file.md`
now opens the document without walking the repo first, and the ~95 ms of pre-window
work that used to cost is measurably gone.

### What happened

**The problem, restated.** `dreamd file.md` is meant to be a Preview-style "open
this one document" gesture. It wasn't. `main()` ran `markdown_paths` →
`SearchIndex::build` → `build_tree` unconditionally *before* `tauri::Builder` was
even constructed, so the walk blocked window creation rather than merely first
paint. Then `init()` awaited `loadTree()` before it asked `initial_file` whether
there was a document at all, so the frontend built and painted the entire sidebar
DOM before rendering the thing the user actually asked for.

1. **`src-tauri/src/catalog.rs` (new).** The tree and the index come from one walk
   and share one lifecycle, so they became one type rather than two independent
   `Mutex`es: `Mutex<Option<Built>>` + `Condvar`. `build` fills it and wakes every
   waiter, `rebuild` re-walks and replaces (the watcher path), `wait_tree` /
   `wait_query` block until it exists. The walk happens *outside* the lock, so
   waiters are only blocked for the handover.
2. **Perf marks moved into the module.** `walk_done` / `index_built` / `tree_built`
   are emitted inside `catalog::walk`, so the synchronous and the deferred path
   report identical phases. Safe from a background thread: `perf::mark` reads a
   global origin set in `main`'s first statement and writes one line under the
   stderr lock, so a late `walk_done` is still a correct timestamp in one
   uncorrupted NDJSON stream.
3. **`main()` keys off `initial.is_some()`.** A directory, no argument, or a
   *non-markdown* file all build synchronously exactly as before — the last of
   those matters, because it yields `initial_file: None` and therefore an empty
   reading pane where the sidebar is the only usable surface. Only a markdown file
   argument spawns the build on a background thread.
4. **`--bench-startup` returns before the spawn** on the deferred path. Exiting
   into a live walk thread would have had hyperfine timing a teardown racing it.
   The flag's doc comment says so.
5. **The three catalog commands became `async`** and do their waiting inside
   `tauri::async_runtime::spawn_blocking` — `list_markdown_files`, `fuzzy_search`,
   `rebuild_index`. Whether Tauri 2 runs sync commands on the main thread wasn't
   worth resolving; this is correct either way. No frontend change was needed:
   `invoke()` already returns a promise, and the palette's existing `paletteSeq`
   guard already handles out-of-order resolution, so opening it before the catalog
   is ready simply waits.
6. **`ui/app.js` `init()` reordered.** `initial_file` moved to the front because it
   decides the sidebar, `loadTree()` is started without being awaited (with a
   `.catch` — unawaited, a rejection there is an unhandled one), and `openFile`
   runs against it. A late tree was already safe: `paintTree` ends with
   `markActiveInTree(currentFile)`, so a tree arriving after `openFile` still marks
   the open document.
7. **The sidebar ships collapsed in markup** — `<body class="nav-collapsed">` — and
   `init()` *removes* the class when there is no initial file. That direction round
   makes the single-file case deterministically flash-free; a directory launch gets
   its sidebar a few ms into JS boot, invisible against the time the window took to
   exist. The class, its CSS and `#btn-expand` all already existed.
8. **`perf/scripts/startup.sh` gained a `prewindow/single-file` hyperfine case.**
   Without it the sweep only ever measures the directory path and this change is
   invisible to it.
9. **`ui-check.mjs` covers both branches now**, 33/33. The existing page stubs
   `initial_file: null`, so it only ever exercised the sidebar-*open* branch; a
   second page stubs a real initial file and a deliberately-late
   `list_markdown_files`, then asserts the document paints while the tree is still
   absent and that the tree marks the open file active when it lands.
10. **`docs/todo.md`** lost its "Startup: optimize for single-file launch" section —
    it's a queue, not a log.

**Deliberately not touched:** `fs_walk::markdown_paths` stays sequential. Moving the
walk off the critical path does weaken its "thread-spawn cost isn't worth it on
small repos" comment, but switching to `build_parallel()` is a separate change that
deserves its own measurement.

### Mistakes & deviations

- **`find | sort | head -1` killed `startup.sh` silently.** Added to pick a corpus
  document for the new hyperfine case. `head` exits after one line, `sort` dies on
  SIGPIPE, and under `set -o pipefail` + `set -e` that took the whole script with
  it — exit 141, *no stderr at all*, and the first `perf-pass` came back with
  `real.startup: null` rather than an error. Caught by noticing the REAL APP section
  had loop metrics but no startup ones. Replaced with `${var%%$'\n'*}` on the full
  sorted list and re-ran the tier; that's why there are two `pass-*.json` files from
  this session.
- **`openFile`'s error handling was nearly dropped.** The old `init()` wrapped
  `initial_file` + `openFile` in one `try/catch`; the plan's replacement had a
  `.catch` on the invoke but left `await openFile(f)` bare, which would have made a
  render failure reject `init()`. Restored.
- **A throwaway `examples/catalog_check.rs` was written and deleted.** It asserted
  that `wait_query` actually blocks on a pending catalog (369 ms against a 300 ms
  deferred build) and that `rebuild` agrees with `wait_tree`. Kept out of the tree
  because the plan didn't ask for a fourth permanent harness.
- The plan named a branch `claude/performance-improvements-todo-pedjj7`; ignored in
  favour of CLAUDE.md's straight-to-main rule.

### State

`cargo build` passes plain and with `--features perf`. `locate_check` 611 fixtures
unmoved, `config_check` 25/25, `theme_check` 10 palettes, `ui-check.mjs` 33/33.
`cargo bench --bench walk` / `--bench search` unmoved, as expected — the walk isn't
faster, only moved.

`perf-pass` (`perf/results/pass-8a2df48-20260725-105523.json`): 195 metrics, **0
regressed**. The two "slower" rows are Chromium scroll/highlightMode and unrelated
to this change. New `prewindow/single-file` is **5.3 ms** against
`prewindow/repo-5000` **100.5 ms** — ~95 ms of pre-window work off the critical
path on a 5000-file repo, which is the win. End-to-end `first_paint` moved
2104 → 2043 ms, but that run's spread was 222 ms and debug first paint is dominated
by the 2 MB markdown render, so the launch-level number is **unconfirmed**; the
pre-window number is the one to quote.

`ipc_tree` **has changed meaning**: it is now marked inside `loadTree()` after
`paintTree`, so it records when the sidebar painted (110 → 1956 ms) rather than a
cumulative boot timestamp. Not a regression, and not comparable to the old baseline.

Left open: the GUI was never driven by hand — no `cargo tauri` in this environment.
`startup.sh`'s real launches do exercise the deferred path end-to-end in WKWebView
(first paint reached, tree landed at 1956 ms), but "open the palette instantly after
launch" and "touch a file → tree repaints" are only covered by the Chromium stub.
`perf-plan.md` is still sitting untracked in the working tree.

## 2026-07-25 — settings panel, ten themes, a writable config

Asked for a settings/preferences panel: interactive keybinds, interactive colour
and font settings, a set of popular themes shipped with the app, a savable custom
theme, and all of it reachable from the command line through TOML pointing at a
theme CSS file. Landed in two commits plus a docs refresh.

### What happened

**Shape of the design, settled in plan mode.**

- *Persistence.* The panel has to save, and tenet 2 said nothing persists. Rather
  than smuggle it in, tenet 2 was rescoped: session state (highlights, annotations,
  the stack) still dies with the process; *preferences* persist to
  `~/.config/dreamd/`. Tenet 1 gained the matching qualifier — never the user's
  markdown, never anything inside the repo.
- *Theme format.* Pure CSS, not a TOML palette that generates CSS. A second source
  of truth for colours alongside `theme.css` would have fought tenet 5. So
  `ui/theme.css` was split: it keeps the **rules**, and a **palette** is a bare
  `:root` block appended after it. `theme = "nord"` picks a palette;
  `theme_css = "/path"` still replaces the whole stylesheet with no palette
  appended, which is what a hand-written file wants.
- *Two commits*, Rust first so the whole feature was exercisable from a shell
  before any UI existed.

**Commit 1 — `d41b10e`, the Rust half.**

1. Ten palettes in `ui/themes/`, `include_str!`'d: dreamd, gruvbox-dark/light,
   catppuccin-mocha/latte, tokyo-night, nord, solarized-light, and a
   high-contrast dark/light pair. Each carries colour, typography
   (`--font-size`, `--line-height`, `--content-width`) and `--syntax-theme`.
2. `--syntax-theme` is the fix for a bug nobody had filed: code-block colours are
   inline styles produced by syntect in Rust, not CSS, so every light theme kept
   dark code blocks. `markdown::render_with` takes the theme name;
   `render(source)` delegates, so `benches/render.rs` and `examples/render_doc.rs`
   were untouched.
3. `config.rs` rewritten. Layering moved from struct merging to raw `toml::Table`
   merging — see *Mistakes* below for why that was not cosmetic. Writes patch the
   global table and rename over the file, so unknown keys and hand-set values
   survive; comments do not, which Oliver chose explicitly over taking a
   `toml_edit` dependency.
4. `theme.rs` grew into the palette registry: `BUNDLED`, the user themes
   directory, `resolve`, `save_user`/`delete_user` with name validation, and
   `syntax_theme` parsed out of the CSS the same way `background` already was.
5. New `cli.rs`: `dreamd theme list|set|show|new`, `dreamd config
   path|edit|get|set`, and `--theme` for one run. They branch out of `main()`
   after the config read and before the walk, so `dreamd theme list` costs a file
   read rather than a repo scan. `args_conflicts_with_subcommands` keeps the
   optional positional working — `dreamd ./theme` still opens a directory of that
   name.
6. `AppState.config` became `Mutex<Config>`, the first configuration in the app
   that changes after startup.
7. `config_dir()` now prefers `$XDG_CONFIG_HOME`, then `~/.config`, and only then
   `dirs::config_dir()`. On macOS `dirs` resolves to
   `~/Library/Application Support`, which is neither what the README promised nor
   where a tmux + Neovim user looks. Nothing existed at either path, so the switch
   cost nothing.
8. The watcher now also watches the user themes directory and canonicalizes paths,
   since FSEvents reports resolved ones and a theme under `/tmp` would never have
   compared equal.

**Commit 2 — `e0751e6`, the panel.** `#settings-overlay` on the existing
`.modal-overlay` / `.open` convention, three tabs. Keys records bindings, flags
duplicates (the global handler is an if-chain, so a duplicate silently makes the
later action unreachable), and labels anything a repo-local `.dreamd.toml`
shadows — the panel writes globally, and without that label it would appear to
save a value that never takes effect. Themes previews live through a second
`<style>` after `#user-theme`, so cancelling is one assignment. Custom edits
variables by targeted string replacement rather than regenerating the block, so
comments and hand-written rules survive.

Two hardcoded shortcuts moved into the keymap: the bare `h` highlight alias
(`quick_highlight`) and `Ctrl+Y` to save an annotation. `--content-width` and
`--ui-font-size` were declared by every palette but never consumed — `#content`
had a hardcoded `820px`. The boot fallback colours in `index.html` had drifted
from the default palette (`--accent` `#4c8bf5` vs `#6ea8fe`, plus four others)
and were realigned; they are what paints the window before the theme arrives.

**Three correctness harnesses**, in the `locate_check.rs` style — this repo has no
`#[cfg(test)]` blocks and that convention was kept rather than contradicted:
`examples/config_check.rs` (25 assertions over layering, the merge bug and
write-back), `examples/theme_check.rs` (every palette declares every variable and
names a real syntect theme), and `perf/harness/ui-check.mjs` (28 assertions
driving the panel in Chromium behind a stub that behaves like the Rust commands).

### Mistakes & deviations

1. **A latent config bug, found while rewriting the merge.** `Config::merge`
   assigned `self.tmux_autodetect = local.tmux_autodetect` and
   `self.keymap = local.keymap` unconditionally. Because `read_file` deserializes
   with `#[serde(default)]`, a `.dreamd.toml` containing only `tmux_target = "%3"`
   produced a fully-defaulted `keymap`, which then overwrote the user's global
   bindings — in that repo only. Not a hypothetical: it would have silently reset
   every shortcut. There is no cheap patch, because the information needed is
   destroyed at deserialization; merging raw tables is what fixes it, and
   `config_check` now pins the behaviour.

2. **A security hole I was about to widen.** `.dreamd.toml` is repo content —
   you get it by cloning — and the plan had it able to set `theme_css`, which
   reads an arbitrary file and injects it into the webview as a `<style>` tag.
   The CSP still permits `https:` in `img-src`, so a `background-image: url(…)`
   turns that into read-and-exfiltrate. Repo-local config may now name a `theme`
   but not set `theme_css`. Flagged by the planning agent, not by me.

3. **The panel's theme editor came up empty**, caught by `ui-check.mjs` on its
   first run. The JS palette parsers matched the `:root { --bg: … }` *example*
   inside `ui/theme.css`'s own header comment, so `paletteBlock` returned the
   documentation instead of the palette. The Rust side already had
   `strip_comments` for exactly this; the JS side now mirrors it. This is the one
   bug that a build, a diff read, and both Rust harnesses would all have missed.

4. **I could not look at the app.** `cargo tauri` is not installed on this
   machine and `screencapture` is blocked, so the plan's "run it and click
   through" verification was not available. Substituted `ui-check.mjs`, which
   turned out to be the better instrument — but nobody has yet seen the panel
   rendered in WKWebView, only in Chromium.

5. **Nearly reported someone else's perf wins as mine.** The first `perf-quick`
   showed `render/code/512k` down 78% and `reanchor_today/10` down 85% on a change
   that touched neither. `perf/baseline.json` is from `cdf24d2`, before the
   optimization pass in `56143c6`, and has not moved since.

### State

`cargo build` clean. `config_check` 25/25, `theme_check` 10/10, `ui-check` 28/28,
`locate_check` unchanged (0 wrong with context). CLI verified by hand against an
isolated `XDG_CONFIG_HOME`: a hand-written config with comments round-trips with
only the intended key changed, no defaults spelled out, and a repo-local file that
omits `[keymap]` no longer resets it.

`perf-pass` at the panel commit: 195 metrics compared, **0 regressed**, 1 slower
(`chromium.palette.repo5000.keystroke_ms.p95`, 0.4 → 0.5 ms — sub-millisecond, and
Chromium-relative rather than WKWebView). The save-to-paint loop is unmoved:
`events_per_save` 1.125 and p95 ~2060 ms match the run at `4138e62`. The 52
improved rows are the earlier optimization pass showing against a stale baseline,
not this work.

Open:

- Nothing has been seen in WKWebView. Worth one look at the panel and at a light
  theme, particularly the high-contrast pair.
- `perf/baseline.json` is stale by two sessions and makes every tier run noisy.
  Fixing it needs a deliberate `perf-deep --update-baseline`.
- Bundled palettes are `include_str!`'d, so editing `ui/themes/*.css` needs a
  rebuild in a release build; debug builds read them off disk to compensate.
- Changing `theme_css` at runtime does not re-arm the watcher — it still watches
  the path resolved at startup. A restart picks it up.
- `code-block-highlights-never-paint` is untouched by this work, but the Custom
  tab's `--hl` swatch will look inert inside fenced code for the same reason.

## 2026-07-25 — chromeless top bar (attempted, reverted)

Tried removing the `#titlebar` row entirely so the document reaches the top edge
of the window, with the three actions (highlight mode, stack, send) becoming
floating buttons over the content. Built, then **reverted before commit** —
Oliver wants the top bar for now. Recorded here so the next attempt starts from
what was already worked out rather than from scratch.

### What the change was

All in `ui/index.html`; no Rust, no `app.js` — the buttons kept their ids, so
every handler and keybind carried over untouched.

- `#titlebar` deleted from both the stylesheet and the markup. `<body>` becomes
  just `#workspace`, so `#content-scroll` starts at window y=0.
- The three buttons moved into `#float-actions`: `position: fixed; top: 8px;
  right: 12px; z-index: 30`, each given `box-shadow: 0 2px 8px rgba(0,0,0,0.4)`
  so they stay legible over text scrolling beneath them.
- `#stale-rail` moved from `top: 8px` to `top: 46px`, otherwise stale chips land
  underneath the floating cluster.

### The three things that made it non-trivial

1. **Window dragging.** `data-tauri-drag-region` lived on `#titlebar`; deleting
   it leaves no way to move the window. Putting a drag strip across the top of
   the document is wrong — it would eat text selection in a reader. The fix was
   to move the drag region onto `#sidebar-header`, which is chrome rather than
   content.

2. **The macOS traffic lights.** `titleBarStyle: "Overlay"` means they float over
   whatever is at the top-left. With the titlebar gone that is `#sidebar-header`,
   so it needed `body.mac #sidebar-header { padding: 10px 8px 10px 82px;
   min-height: 38px }` to clear them and stay tall enough to contain them. With
   the tree collapsed the sidebar is 0px wide and neither problem has a host at
   all — that needed a separate 78×38 invisible `#drag-patch` pinned to the
   top-left corner, which restores dragging and costs no readable text because
   the lights were already covering that corner.

3. **Collision with the stack panel.** `#stack-panel` is pinned top-right and the
   floating cluster would sit on top of its close button.
   `body:has(#stack-panel.open) #float-actions { transform: translateX(-348px) }`
   slid it clear. Fine in WKWebView on this machine, but `:has()` is the kind of
   thing to re-check if the webview floor ever moves.

### Known rough edges if this is picked up again

- Text scrolls *under* the floating buttons. Inherent to the approach; the
  shadow and opaque button background make it legible rather than clipped, but
  it is a real change to how the top of the page reads.
- Tree collapsed on a narrow window puts the traffic lights over the first line
  of the document.
- Never run against the app — no `cargo tauri dev` pass, no `/perf-quick`. The
  change is CSS-and-markup only, but "it looked right in the file" is all the
  verification it ever got.

## 2026-07-25 — kill the white flash on boot

Single-bug thread. The reading pane painted white from window creation until the
frontend finished booting — longer on a slower start, which is exactly backwards.
Fixed in four layers, outermost first.

### What happened

1. **The cause is where `--bg` lives.** `body { background: var(--bg) }` is in
   `ui/theme.css`, and theme.css is injected into `#user-theme` from JS. So for
   the first stretch of boot the variable is simply unset and the webview paints
   its own default white. Nothing was "flashing" — the app had no background yet.

2. **`src-tauri/src/theme.rs` (new).** `resolve()` (user's theme file, else the
   bundled `DEFAULT_THEME`) and `background()`, which parses `--bg` out of the CSS
   as `(r, g, b)`. Hex only — `#rgb`, `#rrggbb`, and the 4/8-digit alpha forms, with
   `/* … */` blocks stripped first so a commented-out declaration cannot win.
   Anything else returns `None` and the caller keeps its default.

3. **The native window is painted from the *user's* theme, not a constant.**
   `main.rs` computes the colour before the builder and `setup` calls
   `set_background_color`. Hardcoding the default dark would have made a custom
   light theme flash dark — a worse bug for the person who bothered to write one
   (tenet 5). `backgroundColor` in `tauri.conf.json` is the static default covering
   the frame before `setup` runs; `get_theme_css` now just calls `theme::resolve`.

4. **`ui/index.html` carries fallbacks.** `html`/`body` get
   `var(--bg, #1b1f27)` / `var(--text, #c7cedb)`, matching the `var(--x, fallback)`
   idiom the rest of the structural CSS already uses. This is what covers webview
   first paint onward.

5. **`loadTheme()` moved to the front of `init()`** in `ui/app.js`, ahead of
   `repo_info` and `get_keymap`. Every IPC before it was time a user with a custom
   theme spent looking at the default one. Consequence for perf: the `ipc_theme`,
   `ipc_repo_info` and `ipc_keymap` marks are cumulative timestamps, so their order
   in the baseline changed — a future diff on those three rows is the reorder, not
   a cost.

### Mistakes & deviations

Ran clean. The one judgement call worth recording is that `#1b1f27` now appears in
three places (theme.css, the index.html fallback, tauri.conf.json); only theme.css
is authoritative at runtime, the other two are pre-boot defaults, and both say so
in a comment.

### State

`cargo build` clean. `theme::background` verified differentially through a throwaway
example — default theme, `#fff`, 8-digit hex, a commented-out decoy, `rgb()`, and a
`--sidebar-bg` near-miss all parse as expected; example deleted after.

`perf/run.sh pass` — the run the previous session deferred — **195 metrics compared:
0 regressed, 1 slower, 54 improved**. The single slower row is
`chromium.palette.repo5000.keystroke_ms.p95` at 0.4 → 0.5ms, sub-millisecond and
Chromium-relative, not a real signal. The 54 improvements are almost all
`chromium.highlight.*.apply_ms` (up to −92%) and belong to the anchoring commit
below, not to this change; boot ordering cannot move highlight apply. Full table:
`perf/results/pass-4138e62-20260725-012944.json`.

Not verified visually — the fix was reasoned from where `--bg` is defined and the
build, not from watching a boot. Worth one look on the next real launch.

## 2026-07-25 — highlight anchoring: 31.6% of selections landed on the wrong copy

Bug-hunting thread. The brief was a suspected off-by-one plus a "quote appears
twice" case in `markdown::locate`, estimated at 6.5% of selections. Both were
real; the estimate was low by a factor of five, because the ground truth the
estimate was measured against was itself wrong.

### What happened

1. **Built the harness first.** `src-tauri/examples/locate_check.rs` runs all 611
   corpus highlight fixtures through `locate` and exits non-zero on a wrong
   anchor. The repo had no `#[cfg(test)]` tests and the session log had named
   `locate()` as the obvious first target for one; this is it. It reimplements the
   whitespace-stripping and line lookup independently rather than importing them —
   a checker that reuses the code under test cannot catch it being wrong.

2. **The original ground truth was wrong, which hid most of the bug.** The brief
   defined truth as `locate(source, "", exact_source_quote, "")`, i.e.
   `source.find`. But the generated corpus repeats whole blocks — one sampled code
   block occurs **195 times** — so `find` returns the first copy, which is usually
   not the one the fixture was sampled from. `perf/corpus/gen.mjs` now records
   `lineStart`/`lineEnd` at sample time (GENERATOR bumped to 3; quote, rendered,
   prefix and suffix bytes are unchanged, so bench inputs are identical). Against
   the recorded origin the real pre-fix failure rate is **193/611 = 31.6%**, not
   6.5%.

3. **Class (b), the off-by-one, was tier 2's bug, not tier 3's.** Tier 3 maps to
   the first and last *non-whitespace* character; tier 2's `span()` used the raw
   byte range, so a selection ending on a line break claimed the following line.
   Tier 3's answer is the right one. `locate` now trims the quote up front and all
   three tiers agree by construction.

4. **Class (a), the wrong copy, needed four separate fixes.** `ui/app.js` sent
   `prefix: ""`, so nothing could disambiguate:
   - `selectionContext()` in `ui/app.js` walks out node by node from the selection
     for 96 chars each side. Deliberately not a Range back to document start —
     `toString()` on that would build a copy of the whole file.
   - Tier 3 became context-aware. Rendered context has lost the markdown syntax
     the source still carries, so occurrences are *scored* by shared bytes either
     side rather than required to match exactly; a full match short-circuits, so
     the common case still stops at the first hit.
   - Tier 2 now runs **only** when there is no context. With context it was
     actively harmful — it took the first exact hit while ignoring the context
     saying otherwise. Worth 10 of the residual failures.
   - `str::match_indices` was replaced by an overlapping-aware `occurrences()`.
     Non-overlapping iteration cannot see a match that begins inside the previous
     one, and periodic text (a repeated config block) is exactly where the right
     copy hides. The last 10 failures.

5. **A position hint settles what context cannot.** Two byte-identical copies of a
   block have byte-identical context; no anchor can choose between them. But a
   re-anchor knows where the highlight was a moment ago. `SourceIndex::locate_near`
   takes the previous line, `Store::reanchor_file` passes it, and among full
   context matches the nearest to the hint wins. One pass, early-exit once it has
   bracketed the hint.

6. **Kept the bench row names, corrected the story.** `benches/locate.rs` led with
   "context is the obvious-looking fix that **does not work**" — true about
   *speed*, and it reads as an argument against the correctness fix. Rewritten.
   The row ids (`today`, `with_context`, …) were left alone so the committed
   baseline still lines up, with a note that `today` is now the pre-fix reference.
   `seed_highlights` in `main.rs` now seeds collapsed context, since that is what
   the app sends.

### Mistakes & deviations

- **Trusted the brief's ground truth.** The first harness reproduced 40/611 as
  advertised, then the fix appeared to make things *worse* (96 mismatches). It
  hadn't: the "correct" answers were wrong. Caught by dumping every occurrence of
  one failing quote and finding 195 of them. Cost a rebuild of the fixtures and
  the harness, and was the single most useful thing in the session — the real
  failure rate is five times what was reported.
- **First hint implementation started the scan at `hint − 4096` bytes and took the
  first full match.** Two bugs: it returned the first match rather than the
  *nearest*, and its "don't rescan from the top" shortcut silently discarded the
  hint for any highlight near the top of a file. It also needed a second pass when
  no full match existed, which is the common case in real markdown. Replaced with
  a single hint-aware pass. 74 highlights still moved on re-anchor before this;
  0 after.
- **`is_none_or` broke MSRV.** Stable since 1.82, `rust-version` is 1.77. Caught by
  `cargo clippy`, not by the build.

### State

`cargo build` clean, `cargo clippy --all-targets` clean apart from one pre-existing
`while_let_loop` in `render`. `locate_check`: 611 fixtures, **0 wrong** on first
anchor, **0 moved** on re-anchor, 0 batched-vs-one-shot disagreements. 146 remain
*ambiguous* — another copy exists where quote and context both match in full, so no
anchor built from that evidence can choose; that is a synthetic-corpus artifact and
re-anchoring resolves all of them via the hint.

`perf/run.sh quick` after the fix: 49 metrics, **0 regressed, 11 improved**;
`locate_single/with_context`, the path the app is now on, 8.23 → 6.66ms. Caveat: the
committed baseline predates the perf commit already in the tree, so that run confirms
no regression rather than isolating this change. **`perf-pass` was skipped at the
user's request** — it should run before the next commit touching `src-tauri/` or
`ui/`.

## 2026-07-25 — the first optimization pass, measured end to end

The measurement framework built last session finally got used for what it was for.
Worked the ranked fix list plus a fresh audit of the hot paths; landed six changes
across Rust and the frontend. Final `perf/run.sh pass`: **0 regressed, 53 improved**.

### What happened

1. **`reanchor` no longer rebuilds its index per highlight.** `markdown::locate`'s
   third tier built a whitespace-stripped copy of the entire source plus an offset
   table, and threw it away again — *once per highlight*, against documents that can
   be megabytes. New `markdown::SourceIndex` builds it once per `reanchor_file` call
   and lends it to every highlight in the file. Two further scans went with it: line
   numbers now come from a prefix-summed line table (`partition_point`) instead of
   counting newlines from byte 0 twice per highlight, and the stripped index is keyed
   by *byte* rather than char index, which removed a `chars().count()` over the whole
   document per lookup. `reanchor_today/100` 702.7ms → 107.8ms; `locate_single/today`
   improved too (8.08 → 6.85ms), so nothing was traded away to get it.

2. **Fenced code blocks are highlighted in parallel.** Syntect is essentially all of
   render cost — `render/code/2m` was 3033ms against `render/prose/2m` at 4ms — and
   the blocks are independent of each other. They're now collected during the parse
   pass, highlighted across the cores with `std::thread::scope` and an atomic index
   (work-stealing, because block sizes vary hugely), then spliced back into their
   event slots. No new dependency. `render/code/512k` 779.6 → 176.3ms,
   `render/mixed/512k` 130.0 → 33.8ms. Output verified **byte-identical** on all four
   corpus variants before the benches were believed.

3. **The watcher debounces and coalesces (fix B2).** 60ms window, per path. The
   subtlety: kinds are *accumulated*, not overwritten, and resolved against the
   filesystem at flush — an editor that saves by writing a temp file and renaming it
   over the original emits a remove *and* a create for a file that still exists, so
   collapsing to "removed" would have blanked the open document. `events_per_save`
   1.583 → **1.00**.

4. **The startup double walk is gone (fix B4).** `main` builds the tree from the
   paths it already walked for the search index and caches it in `AppState`;
   `list_markdown_files` returns the cache, and `rebuild_index` returns the fresh
   tree so the frontend stops asking for a second walk on every add/remove. Also
   dropped a `stat` per repo entry: the walker filtered on `into_path().is_file()`,
   which discarded the `DirEntry`'s already-cached `file_type()` and issued a fresh
   syscall for every entry — directories included — *before* the cheap extension test
   could reject it. `walk_startup_pair/5000` 118.7 → 57.4ms.

5. **`applyHighlights` stopped re-walking the document per highlight.** It built a
   fresh `TreeWalker` from the top of a 105k-node document for each one. Now one pass
   flattens the document to a string plus an offset table, quotes are found with a
   native string search, and the wraps are applied back-to-front so splitting a text
   node can't invalidate an offset computed after it. Kept a walk-and-stop path below
   five highlights — flattening costs ~4ms whether there is one quote or five hundred,
   and the measured crossover is around five. Chromium apply at 500: 284.9 → 23.9ms.
   Applied-counts match the baseline exactly, so behaviour is unchanged.

6. **Frontend odds and ends.** The active tree item is tracked by reference instead of
   toggling a class on all 5000 file nodes per file open; palette arrow keys move a
   class instead of rebuilding 200 rows (2.10 → 0.60ms); `runPalette` got a monotonic
   sequence guard so a slow query can't paint over a newer one; `escapeHtml` does one
   regex pass instead of four; the capture-phase scroll listener is passive and
   returns immediately when no tooltip is showing.

### Mistakes & deviations

- **Two ideas were measured and rejected, not shipped.** The parallel repo walker
  (`build_parallel`) is ~45% faster on 5000 files but pays ~1.2ms of thread spawn that
  a small repo eats in full — 3.5x slower on `walk/markdown_paths/10`, which is exactly
  what a cold start on a single file hits. Kept the sequential walker with only the
  `stat` fix, which is a strict win at every size. `content-visibility: auto` cuts
  forced layout after a render by 97% but raises scroll main-thread time by 81%;
  scoping it to `> pre` was worse on both axes, and an A/B with the rule removed
  confirmed the cost was entirely its own. Scrolling is the dominant interaction in a
  reader, so it was left out — deliberately, with the numbers recorded in a comment in
  `index.html` so nobody re-litigates it blind. Worth re-testing under WKWebView.
- **A first cut of the stripped index used two offset tables and a binary search.** It
  made batch re-anchoring fast but left single `locate` calls ~14% *slower* than
  before. Rewriting it as one byte-keyed table made the lookup O(1) and turned that
  regression into an improvement. Caught because the single-call bench was read
  alongside the batch one, not instead of it.
- **Three rows were flagged as regressions and turned out to be noise.**
  `render/table/8k` (+54%), `render/table/512k`, and `locate_single/with_context`
  (+16%) all moved on a loaded machine, had no plausible mechanism in the diff — the
  table variant contains no code blocks at all — and came back flat or improved on a
  quiet re-run. The repo's own rule held: a flagged row needs a mechanism or an A/B
  before it gets called a regression.
- **Started this thread on the assumption the tree was dirty and non-building** (a
  rustc diagnostic reported `copy_clipboard` as private). It was stale; a parallel
  thread had already committed that work. Confirmed against `git log` before doing
  anything, rather than "fixing" a file that was already correct.

### State

`cargo build` passes. `perf/run.sh pass` on the shipped tree: 195 metrics compared,
**0 regressed, 1 slower, 53 improved** — the one slower row is Chromium `composite_ms`
moving 3.3 → 3.7ms across 30 wheel ticks, well inside the harness's own threshold and
relative-only anyway. Rendered HTML byte-identical on four corpus variants; highlight
applied-counts identical to baseline.

`perf/baseline.json` is **not** updated — that needs a deliberate `perf-deep
--update-baseline` run on a quiet machine, in its own commit. Until then every `real.*`
row will keep reporting as new.

**A pre-existing correctness bug was found and deliberately left alone.** Verifying
`locate` against all 611 corpus highlight fixtures showed **40 cases where a rendered
quote anchors to the wrong lines** — one resolved to lines 108–113 instead of 362–368.
Confirmed identical before and after this session's changes, so it is not a regression.
Root cause lead: `ui/app.js:419` sends `prefix: ""` and `suffix: ""`, so tier 1 of
`locate` can never fire and both remaining tiers take the *first* match in the
document. A handoff prompt for the fix was written this session. Note that populating
prefix/suffix was previously rejected as a *performance* fix — that finding stands and
does not argue against it here, since disambiguation is exactly what it is for.

Left on the table, ranked: a render cache keyed on (path, mtime, hash) to kill
re-renders of unchanged content; not holding the store mutex across `reanchor`;
raw-bytes IPC instead of a JSON-escaped 4MB string.

## 2026-07-25 — dreamd gets a public face at fongo.uk/dreamd

Built and shipped the project's first website: a single dark, picture-free landing
page live at `https://fongo.uk/dreamd`, in a new `website/` directory that is its own
Astro project and its own deploy. Nothing in `src-tauri/` or `ui/` was touched. Also
fixed a canonical-URL defect that turned out to affect autorota too, and wrote
`website/CLAUDE.md` as the source of truth for the directory.

### What happened

1. **Copied autorota's hosting pattern, and corrected the premise while doing it.**
   `website/` deploys as an assets-only Cloudflare Worker (`dreamd-web`) on the zone
   route `fongo.uk/dreamd*`, which intercepts that one path. The rest of fongo.uk is
   **paper_web on Vercel** — autorota's `wrangler.jsonc` claims Cloudflare Pages and
   is stale. There is no build-time wiring between the repos at all: no submodule, no
   symlink, no sync script, just the zone route plus a hardcoded `link` string in
   paper_web's `project_list.json`. The load-bearing trick is `base: "/dreamd"` with
   `outDir: "./dist/dreamd"` while wrangler's `assets.directory` is `./dist` — Astro's
   `base` only prefixes URLs, not output paths, so this is what lines the Worker's 1:1
   path→asset mapping up with the route prefix.

2. **Design decisions, all deliberate and recorded in `website/CLAUDE.md`:** dark only
   (no toggle, no `data-theme` — a divergence from paper_web and autorota, which are
   both dual-theme); tokens lifted verbatim from `ui/theme.css` so the site is the
   app's own colours, pushed toward near-black; Spectral **500 only** for display with
   the app's system-sans stack for prose; no images; and the app's highlight yellow as
   the page's only visual device, used three times total. Copy says "Source on GitHub"
   and never "open source" — the repo has no `LICENSE` and no `license` field in either
   `Cargo.toml`, so it is legally all-rights-reserved.

3. **The landing went through three shapes.** First a hero-scoped drifting gradient;
   then, on a brief for a Sandman-like dream aesthetic, a darker page-level field
   (starfield, indigo and violet veils on unrelated clocks, a breathing teal aurora,
   sand grains in the highlight yellow); finally scoped back to a **sticky one-screen
   landing** that the opaque `.page` scrolls up over like a curtain, so the site reads
   as two pages joined by a scroll. Star density was cut ~73% on request. All animation
   is transform/opacity only, and everything freezes under `prefers-reduced-motion`.

4. **The name became the title.** `dreamd` set large in Spectral italic, the former
   headline demoted to subhead. The corner wordmark is hidden on the landing — the name
   is already the headline there — and arrives past 70% of a viewport as the way home.

5. **`website/CLAUDE.md`** documents the wiring, the invariants, the zero-JS-bundle
   contract, the verification checklist, and every trap below. A pointer line was added
   to the root `CLAUDE.md` under Docs so the directory is discoverable from the root.

6. **Canonical URLs, in both repos.** `/dreamd` 307'd to `/dreamd/` while the page's own
   `<link rel="canonical">` was the slashless form — the canonical pointed at a
   redirect. The obvious one-word fix (`html_handling: "drop-trailing-slash"`) would
   have fixed dreamd's index and **broken autorota's subpages**, because Astro reports
   `/autorota` for the index but `/autorota/support/` for a nested route, so the
   canonicals already disagreed with each other. The real fix is three settings in
   agreement: `drop-trailing-slash` in `wrangler.jsonc`, `trailingSlash: "never"` in
   `astro.config.mjs`, and a canonical normalisation in `SiteLayout.astro`. Applied to
   both sites and both deployed; autorota's stale Pages comment corrected in passing.

### Mistakes & deviations

- **`overflow-x: hidden` on `body` silently killed the landing's `position: sticky`.**
  It makes `body` a scroll container, so the landing scrolled away instead of pinning
  and the curtain never happened. Caught by asserting
  `.landing.getBoundingClientRect().top === 0` at several scroll offsets rather than by
  looking at a screenshot. `html` does the horizontal clamp now; `body` is left alone.
- **`fullPage` screenshots flatten sticky and fixed layers**, rendering them once at the
  top. This made the whole lower page look flat and dead when it was fine, and I nearly
  "fixed" a non-problem. Switched to viewport-sized shots at explicit `scrollTo`
  offsets, which is the only way to judge these states.
- **The curtain sliced the subhead mid-glyph** — it read as a clipping bug, not a
  reveal. Added a `--wake` variable, written by the existing scroll handler, that fades
  and lifts the landing text to completion by 40% of a viewport, so the rising edge
  arrives at empty sky.
- **Shipped Spectral 400 and then cut it.** Headings are 500 and body prose is system
  sans, so 400 was two woff2 files of dead weight; noticed on the first build's preload
  list and removed, halving the font payload.
- **Trimmed a sentence of hero copy during the restructure without flagging it**, and
  the user wanted it back. Restored minus its leading "dreamd" (the title now says the
  name directly above it), and widened the measure 30em → 34em because at 30em the line
  broke with "action." orphaned and `text-wrap: pretty` did not rescue it.
- **My `cd` in tool calls left the user's shell in `perf/harness`,** so their
  `npm run deploy` failed twice with a confusing "Missing script" error. The shell's cwd
  is shared with the Bash tool.
- **The root `.gitignore` ignores `*.svg` wholesale**, which would have silently dropped
  `public/favicon.svg` and left a fresh clone unable to rebuild the site. Caught while
  staging, because the file count was one short; re-included with a negation in
  `website/.gitignore`.
- **A redirect loop that wasn't.** Immediately after the `html_handling` deploy,
  `/dreamd` and `/dreamd/` both 307'd at each other. It was Cloudflare still serving the
  old cached redirect — a cache-buster query returned 200 straight away.

### State

No Rust and no `ui/` changes this session, so **no `cargo build` gate and no perf tier
were run** — the binary is untouched and measuring it would have proved nothing.
`perf/baseline.json` not touched.

Site verified in Chromium via the existing `perf/harness` Playwright install (a plain
static page, so unlike the app's perf numbers these results are the real thing, not a
proxy): sticky pinned at `top = 0` across offsets; wordmark opacity 0 → 1 past 70% of a
viewport and clicking it returns `scrollY` to 0; landing text fits inside one screen at
1440×900, 1280×700, 375×812 and 375×667 with no clipping; no horizontal overflow at 375
or 1440; under `reducedMotion: "reduce"` zero animations running; no console errors. On
the built output: no `.js` emitted, Spectral `@font-face` and preloads present. HTML
4.47 KB gzip, CSS 1.86 KB, **0 bytes of JS bundle** (one inline scroll listener), fonts
2 woff2 / 32 KB — all well inside paper_web's budgets, which this directory follows as
design rules since it has no perf harness of its own.

Live and checked after deploy: `/dreamd`, `/autorota`, `/autorota/support`,
`/autorota/privacy`, `/autorota/tutorials` all 200 at the slashless form; slashed forms
are a single 307 hop; unknown paths 404; `/` and `/projects` still 200, so the zone route
shadows only its own prefix. Served HTML was byte-identical to the local build. The
cross-site link from paper_web's `/projects` hard-navigates cleanly despite that repo's
`<ClientRouter />` — no paper_web DOM survives, `data-landing` is set, no errors — so no
`data-astro-reload` is needed.

Pushed: `b7c4e19` (the site), `3a51203` (`website/CLAUDE.md`), `e49b101` (canonical fix),
plus the earlier `1e8cd8f` perf commit they carried along. autorota pushed separately as
`6d96125`. paper_web's card was already pointing at `/dreamd` — committed by the user as
`1159ea4` during the session.

Open, and **not** from this session: `src-tauri/` and `ui/` have uncommitted
modifications from parallel work (`benches/walk.rs`, `annotations.rs`, `fs_walk.rs`,
`main.rs`, `markdown.rs`, `watcher.rs`, `ui/app.js`, `ui/index.html`). Deliberately left
alone — staging was explicit throughout. Still open on the site itself: no `LICENSE`, so
the copy cannot claim a licence; a gallery/media tab is scaffolded for but not built (the
`nav` array in `Header.astro` is empty and wired).

## 2026-07-24 — Rust debloat pass

A pure simplification sweep over `src-tauri/`: no behavior changes, no new
features, just deleting duplication. Net −32 lines across `src/`, and one of the
deletions turned out to be worth 16–22% on the whole search path.

### What happened

1. **`fs_walk::scan` and `markdown_paths` were two copies of the same walker.**
   Each built its own `ignore::WalkBuilder`, applied the same `extra_ignores`
   overrides, filtered to markdown and sorted. `scan` is now
   `build_tree(root, &markdown_paths(root, ignores))`. The explicit
   `.git_global(true).git_ignore(true).git_exclude(true)` on the old `scan` were
   deleted because they are `WalkBuilder`'s defaults — they read as meaningful
   configuration and were not. `build_tree`'s nested `Dir`/`to_node` helpers were
   lifted to module level, `rel_of` extracted (it was inlined twice), and the
   manual `comps[..len-1]` + `.last().unwrap()` replaced with `split_last`.
   58 lines gone from that file alone.

2. **`SearchIndex` lost its `by_rel: HashMap<String, usize>`.** `Pattern::match_list`
   is generic over `T: AsRef<str>`, so `impl AsRef<str> for Entry` makes nucleo
   hand back the entry itself instead of a `&str` we then looked up again — the map
   existed only to undo that. This also fixed a latent bug: two files with the same
   `rel` collided in the map and one was silently unreachable. `node()` became
   `impl From<&Entry> for FileNode`. **This is the one change with a measured
   payoff** — see State.

3. **`is_markdown` existed three times**, verbatim, in `lib.rs`, `fs_walk.rs` and
   `watcher.rs`. One copy now, in `lib.rs`.

4. **`Store::stack_pairs` was a character-for-character copy of `selected_pairs`.**
   It's now `self.selected_pairs(&self.stack)`. Added a private `Store::find` for
   the id lookup repeated in `get` and `selected_pairs`.

5. **`markdown::locate` tiers 1 and 2 built the same `Location` twice** with
   slightly different arithmetic; extracted `span(source, start, len)`. Both
   `.unwrap()`s in `render`'s code-block buffer are gone — the guard-then-unwrap
   pattern (`Event::Text(t) if code_buf.is_some() => code_buf.as_mut().unwrap()`)
   became a match on `&mut code_buf` / `code_buf.take()`, which is the same
   semantics with the impossible case expressed as a branch instead of a panic.

6. **`send.rs`**: four `push_str(&format!(...))` calls became `write!` (no
   throwaway `String` per line); two identical tmux exit-status checks became
   `tmux_run(args, what)`; `detect_claude_pane`'s manual `splitn(3)` plus three
   `unwrap_or("")` plus a `format!` became one `split_once` inside a `find_map`.
   `copy_clipboard` was made `pub` because `main.rs` carried a second copy of it.

7. Smaller: `resolve_repo_root`'s hand-rolled parent loop → `ancestors()`;
   `map_or`, `is_ok_and`, nested or-patterns (`Some("md" | "markdown" | ...)`), and
   a dead `cfg.clone()` in `main`.

8. **Ran `cargo fmt` on the repo.** It was already failing `--check` on `main`
   before this session — four bench files and several `src/` files — which is why
   `benches/` appears in the diff of a session that changed no bench logic.

### Mistakes & deviations

The refactor itself ran clean, but the *verification* did not, twice:

- **`perf-quick` reported `reanchor_today/1` and `/10` up 6–9%, reproducibly**,
  across two runs. Reproducible is supposed to mean real. It wasn't: a direct
  criterion A/B of the stashed old code against the new gave −3.3%/−4.2%/+1.8%,
  all `p > 0.05`. The tell was mechanism — `reanchor_file` is untouched, and
  `locate_single/today` hadn't moved — plus `meta.load1` of 4.6 on 8 cores.
- **`perf-pass` then reported `render/code/512k` +14.7%, `render/code/128k` +13.1%,
  `render/table/512k` +15.2%, `walk_scan/500` +7.6%** — past the 15% regression
  line in one case. Same story: re-running the *identical new code* on a quieter
  machine gave 764ms against a 779ms baseline, i.e. −2%, and an old-vs-new A/B put
  every render and walk figure inside ±3%. Machine load, not the edit.

The rule this reinforces: on a loaded machine the tier diff is a screening test,
not a verdict. A flagged row needs either a plausible mechanism in the diff or a
back-to-back A/B against the stashed old code before it gets called a regression.

Because there are no unit tests, correctness was established by differential
testing rather than by the build: a throwaway `examples/_abcheck.rs` dumped
`render` over six real documents, `scan` (with and without `extra_ignores`),
`markdown_paths`, seven `query` cases and seven `locate` cases covering all three
tiers, and `assemble_query` over stale / multi-line / out-of-root / unannotated
highlights. Every byte identical between stashed-old and new. The harness was
deleted afterwards rather than committed — it asserts nothing, it only diffs.

Also corrected a comment my own change had made stale: `benches/walk.rs` said
`markdown_paths` and `scan` "are the two duplicated halves" and that fix B4 is
about merging them. B4 is actually about the *startup pair* — `main()` and
`list_markdown_files` each walking the repo — which this session did not touch.

### State

`cargo build` clean with and without `--features perf`;
`cargo clippy --all-targets --all-features -- -D warnings` clean; `cargo fmt
--check` clean (for the first time). `cargo test` runs **0 tests** — there are
still no `#[cfg(test)]` blocks, so it proves compilation and nothing else.

`perf-pass` measured, results in `perf/results/pass-cdf24d2-20260724-233207.json`:

- **Improved, real, consistent across every size**: `index_build` −18%/−20%/−18%
  (10/500/5000 files), `query/500` −17%, `keystrokes/10` −22%, `keystrokes/500`
  −16%. All of it the `by_rel` HashMap removal.
- **Nothing regressed.** Every slower row was disproven by A/B (above).
- The four `XX` rows were all `chromium.highlight.*.apply_ms`, which is
  `ui/app.js` — zero JS changed this session, the Chromium noise floor is 27%, and
  one of them was a 190% "move" on a single-highlight sample. Chromium-relative
  numbers, not WKWebView.

Open, and **not** caused by this session: the `pass` tier's entire `real.*` group
now has nothing to compare against. Commit `cdf24d2` renamed those metric keys
(`real.loop.h10` → `real.loop.debug-h10`, `real.startup.*` →
`real.startup.debug.*`) so the pass tier emits `debug`-prefixed paths while
`baseline.json` still holds the release-tier names — so they print as `*` (no
baseline) and their old names print under `not measured this run`. That includes
`events_per_save` and `save_to_paint_ms`, which `perf-pass` itself calls the most
product-relevant numbers it has. Needs a `perf-deep --update-baseline`, or keys
that don't encode the profile, before those rows mean anything again.

`perf/baseline.json` untouched, as required.

## 2026-07-24 — performance measurement framework

Built the three-tier performance harness (`perf/`) and captured a first baseline.
No optimizations yet — this session was about being able to prove them. It got
there, but only after the harness was caught lying five separate times.

### What happened

1. **`src-tauri` split into `lib.rs` + a thin `main.rs`.** `main.rs` keeps the CLI,
   `AppState`, the 21 commands and the builder; everything they do moved to the
   library. Code moved verbatim — no logic changed. This is what makes
   `src-tauri/benches/` and future unit tests possible at all, since a `[[bin]]`
   target can't be imported.

2. **Deterministic corpus** (`perf/corpus/gen.mjs`, node, no deps). 5,536 files /
   21MB: four document variants at 8KB–2MB, synthetic repos of 10/500/5000 files,
   highlight sets of 1/10/100/500. Seeded PRNG, byte-identical across runs; a
   3.5KB summary manifest with an aggregate digest is committed rather than 5,500
   individual hashes.

   Highlight fixtures carry both `quote` (exact source) and `rendered`
   (whitespace-collapsed). That distinction turned out to be load-bearing — see
   the findings below.

3. **Four criterion suites** — `render`, `locate`, `search`, `walk` — with sample
   counts capped per group so the deep sweep stays inside its budget.

4. **A `perf` cargo feature** (off by default, verified to compile out) emitting
   NDJSON timing marks on **stderr**, because `console.log` inside WKWebView never
   reaches the process's stdout. Rust marks plus frontend marks forwarded through a
   `perf_mark` command land in one ordered stream. Also `--bench-startup` and a
   `DREAMD_PERF_SEED` hook so the save loop can be measured at a realistic
   highlight count without driving the UI.

5. **Playwright/Chromium harness** (`perf/harness/`, test-only, never referenced by
   `tauri.conf.json`) with a stubbed `window.__TAURI__`. Scroll cost is taken from
   CDP trace events, not `performance.now()` — style, paint, raster and composite
   happen outside JavaScript, so timing a `scrollTop` assignment from inside the
   page reports approximately zero no matter how expensive the scroll is.

6. **Runner, three tiers, one entry point.** quick ~80s, pass ~6min, deep ~20min.
   Metrics are flattened to dot-paths and diffed against `perf/baseline.json`;
   adding a measurement anywhere shows up in the table with no registry to update.

7. **Three skills** — `perf-quick`, `perf-pass`, `perf-deep` — plus a perf clause in
   `wrap-up`'s gate and a line in CLAUDE.md.

### Findings (all measured, release profile)

- `reanchor` costs **~7ms per highlight** on a 2MB document — and a *live* highlight
  costs exactly what a stale one does (7.08ms vs 7.10ms), because a quote taken from
  the rendered DOM never matches `locate`'s cheap tiers and always rebuilds the
  whitespace-stripped index. 500 highlights = 3.9s.
- The watcher emits **1.6 events per save**, and they compound: `reanchor` measured
  at 74ms in isolation becomes 2.5s in the live app because the re-renders serialize
  on the main thread. Save→repaint is **5.4s** at 100 highlights.
- First paint **1,417ms** on a 5000-file repo, 1,189ms on a trivial one. The entire
  pre-window Rust sequence is only 59ms of that.
- 512KB of markdown becomes **1.06MB of HTML** with ~12,000 inline-styled spans.
- Selections spanning inline markup **fail to anchor 76–100%** of the time and are
  silently demoted to stale chips. That is a correctness bug, found incidentally.

### Mistakes & deviations

This thread did not run clean; the harness produced confident, plausible, wrong
numbers five times, and each one had to be caught by disbelieving a result.

- **`grep -q` under `set -o pipefail`** reported every symbolicated binary as
  stripped — `grep -q` exits early, `nm` dies of SIGPIPE, the pipeline reports
  failure despite matching.
- **`$!` on a subshell** meant the kill hit the subshell, not the app, leaking a
  live `dreamd` per run. Fixed with `exec`.
- **A relative binary path after `cd`** made first-paint measurement return empty
  rather than wrong — silence, not an error.
- **Workload mismatches keyed to the same metric path**, three times over: quick
  rendered 512KB while deep rendered 2MB; quick cut `--sample-size` to 10 while deep
  used full sampling; `pass` measured the debug binary while the baseline held
  release. Each produced double-digit phantom deltas — the 250% "regression" in the
  final `perf-pass` was the last of them.
- **Thresholds set below the noise floor.** Two runs on identical code disagreed by
  up to 27% on Chromium raster. 5%/15% guaranteed false positives.
- I also **corrupted three of my own baseline runs** by running `cargo clippy` and
  `gen.mjs --force` alongside them, and by killing leaked processes mid-measurement.

The corrections, and the rule they produced: **tiers differ in how much they run,
never in how they run it**, and anything that differs about a workload belongs in
the metric's key, not silently in its value. Plus a lockfile, load recorded in
`meta`, min-of-3 for launches (first-paint spread went from ~3x to 59ms), and
thresholds measured rather than guessed.

Two items from the plan were corrected by the measurements: adding prefix/suffix
context does **not** fix `reanchor` (rendered context misses tier 1 identically —
7.22ms vs 7.21ms; the fix is memoizing the stripped index once per call), and
deduplicating the double repo walk is worth ~57ms, not the headline it was ranked
as. The debounce fix is the top item, above where it was ranked.

### State

`cargo build` clean with and without `--features perf`; `cargo clippy --all-targets`
clean; all shell and JS syntax-checked. Verified the harness detects the known
missing-debounce bug positively (1.6 events/save where >1.0 is the signal), that
injected +40% and +50% regressions are caught and exit non-zero, and that two runs
on identical code flag nothing.

The final `perf-pass` reported 33 regressions; **all are harness artifacts, not code
regressions** — 21 from the debug-vs-release profile mismatch fixed in this commit,
the rest sub-5ms benchmarks within noise. No measured Rust logic changed this
session, so bench movement is noise by construction.

`perf/baseline.json` is committed from a clean deep run. Its `real.*` entries
predate the profile-keying fix and will realign on the next
`./perf/run.sh deep --update-baseline`; `bench.*` and `chromium.*` are current. The
baseline was deliberately not hand-edited to paper over this.

Nothing optimized yet. The ranked fix list stands, led by watcher debounce,
`reanchor` index memoization, and syntect warm-up.

## 2026-07-24 — icon-button tooltips with keybinds

Every icon-only button in the GUI now shows a hover popup naming the button and,
where one exists, the keybind that triggers it. Frontend-only; landed.

### What happened

1. **Inventoried the icon-only buttons.** Seven: `#btn-hl-mode`, `#btn-stack`,
   `#btn-send` (titlebar), `#btn-collapse` / `#btn-expand` (file tree),
   `#stack-close`, and the per-file `⋯` (`.file-opts`, built in `app.js`).

2. **Dropped native `title` in favour of `data-tip`.** The browser tooltip has a
   ~1s delay, can't be styled, and can't render the keybind chip. Each button now
   carries `data-tip="<label>"` plus an optional `data-tip-key="<keymap field>"`
   (`toggle_stack`, `send_stack`). Storing the *keymap field name* rather than a
   literal combo means a user-configured bind from `get_keymap` renders correctly
   — the tooltip reads the live `keymap` object at show time.

3. **`wireTooltips()` in `ui/app.js`.** One `#tooltip` div, delegated
   `mouseover`/`mouseout` off `document` so tree rows rendered later are covered
   without rewiring. 350ms delay, positioned below the button and flipped above
   when it would clip the viewport bottom, clamped horizontally. Also fires on
   `focusin` (keyboard parity) and hides on click, scroll, and blur — a click
   means the user already knows what the button does.

4. **Styling in `index.html`'s structural `<style>`, not `theme.css`.** The popup
   is chrome, not reading surface; it inherits the existing `--sidebar-bg` /
   `--border` vars so a theme still recolours it.

### Mistakes & deviations

- First cut called `scheduleTip()` without claiming `tipTarget` up front, so every
  `mouseover` bubbling from inside the button restarted the 350ms timer and the
  tooltip never appeared while the mouse was moving. Fixed by setting `tipTarget`
  at schedule time, not at show time.
- The `innerHTML` write tripped a security hook warning; both interpolations go
  through the existing `escapeHtml()`, so it was left as-is.

### State

`node --check ui/app.js` passes. No Rust touched, so no `cargo build` gate. Not
exercised in a running window — the hover behaviour is unverified visually.

## 2026-07-24 — session rituals: wrap-up skill + daily project doc

Ported the blogregator docs setup into dreamd: a `/wrap-up` skill, a
`/update-project-doc` skill, the `engies/project.md` landing page, and a cloud
routine that refreshes that page daily. All landed.

### What happened

1. **Surveyed the source pattern.** Read blogregator's `CLAUDE.md`,
   `engies/project.md`, and `engies/ai-practices.md`, plus the existing wrap-up
   skills in `tree/` and `spotify_interview/`. Found blogregator has no
   `.claude/` of its own — the wrap-up ritual lives in those other repos, and
   what blogregator contributes is the `engies/` convention plus the daily job.
   Also found the blogregator routine creation had **failed** with a 403
   ("You don't have access to a repository this routine uses") — the daily job
   the user believed was running never existed.

2. **`.claude/skills/wrap-up/SKILL.md`.** Review diff → gate (`cargo build`
   only when `src-tauri/` is touched) → prepend a dated section to
   `docs/session-log.md` → refresh `engies/project.md` if the project story
   changed → one atomic commit **straight to main** + push → lean memory →
   report. Log layout decision: keep dreamd's existing single running file and
   prepend newest-first, rather than adopting the `session-logs/` directory the
   other two repos use.

3. **`.claude/skills/update-project-doc/SKILL.md`.** Regenerates
   `engies/project.md` from `git log` + `docs/session-log.md` + `README.md` +
   the source tree. Pins the section contract, the entry-level voice, and an
   explicit *"if nothing meaningful changed, do not manufacture news — leave the
   file untouched and make no commit"* rule, so a daily unattended job can't
   invent progress. Commits only that one path.

4. **`engies/project.md`.** The human landing page for dreamd: product loop,
   module-by-module architecture, honest known limits, glossary, reverse-chron
   "Recent updates".

5. **`CLAUDE.md`.** Terse machine-facing tenets (read-only, nothing persists, no
   shell interpolation of user content, escape-don't-execute, CSS-themeable) plus
   the docs conventions. Human-facing guidance deliberately stays in `engies/`.

6. **Cloud routine.** `trig_01GLUNmetTpUmT5ptfLzrMLM`, cron `3 7 * * *` UTC
   (≈08:03 UK in BST), sonnet-5, tools limited to Bash/Read/Write/Edit/Glob/Grep.
   Its prompt tells the agent to read `.claude/skills/update-project-doc/SKILL.md`
   from the checkout and follow it — so editing the skill changes the job, no
   routine edit needed.

### Mistakes & deviations

- **First routine creation 403'd**, same as blogregator's: claude.ai had no
  GitHub access to `bongofongo/dreamd`. Saved the exact create body to the
  scratchpad, reported the blocker with the fix (connect GitHub at
  claude.ai/code). User updated the Claude GitHub app; the retry returned 200.
- **Test run was inconclusive.** Fired the routine manually and polled
  `git ls-remote origin main` for ~5 min — no new commit. That is the expected
  no-op path (project.md was written the same day from the same git log), but
  the cloud session's transcript isn't readable from the CLI, so *correct no-op*
  and *failed run* look identical from here. Reported it as unproven rather than
  claiming success. Real verification comes at the next scheduled run.

### State

Docs/skills only — no Rust touched, no build gate needed. Skills committed and
pushed to main (`b78c9fb`). Routine created and enabled, next run
2026-07-25 07:03 UTC. `engies/project.md` left as written earlier this session;
its top "Recent updates" bullet already covers this work.

## 2026-07-24 — v1 build

Went from an empty scaffold to a working v1 of dreamd in one session.

### What happened

1. **Plan review.** Attacked the original `docs/plan.md` for gaps: the tmux
   send-to-Claude design (injection/escaping), raw-HTML XSS in the webview,
   scroll loss on live reload, missing link/image handling, no launch CLI, and
   the "Telescope reuse" assumption. Reworked the design around a
   highlight → annotation → **stack** → send loop (annotations promoted from v2
   to v1 core; nothing persisted — in-memory, dies with the process).

2. **Backend (`src-tauri/src/`).**
   - `fs_walk` — `ignore`-crate markdown scan → `FileNode` tree.
   - `markdown` — `pulldown-cmark` + `syntect`; raw HTML **escaped** (XSS closed);
     `locate()` powers both anchoring and evidence `file:line`.
   - `annotations` — in-memory highlights/annotations/stack + re-anchoring
     (Active → Stale when the highlighted text itself is edited).
   - `search` — `nucleo` fuzzy over file paths (Telescope lookalike).
   - `send` — assemble a temp query file; auto-detect a `claude` tmux pane and
     type a fixed `read @file` prompt (no shell interpolation), else clipboard.
   - `watcher` — `notify` → `file-added/changed/removed` + `theme-reloaded`.
   - `config` — TOML global + repo-local `.dreamd.toml` override.

3. **Frontend (`ui/`).** Tree, fuzzy palette, stack panel, annotation modal,
   live highlight wrap, stale margin rail, scroll-preserving reload, link/image
   resolution, embedded hot-reloadable theme.

4. **Security fix.** Restricted `open_external` to `http`/`https`/`mailto`;
   stopped routing bare local paths to the OS opener; gated relative images to
   inside the repo root.

### UI iterations (same session)

- Fixed viewer scrolling (grid row was unbounded).
- Highlight mode: highlighter icon toggles auto-highlight-on-select; `h`
  highlights the current selection and prompts for an annotation.
- Collapsible panes; edit existing highlights by clicking them (re-add / edit /
  delete), which is also how a removed stack pair gets re-added.
- Overlay titlebar (macOS) so **highlight · stack · send** icons sit on the
  traffic-light row; file path removed from the top bar; repo root shown
  home-relative (`~/…`) in the tree header.
- Collapse arrow: `◀` in the tree header when expanded, floating `▶` when
  collapsed; preview always full-width.
- Per-file `⋯` menu: Copy path / Delete (moves to OS Trash, repo-scoped, with a
  confirm dialog).
- `Ctrl+Y` submits the annotation from the textarea (keyboard-only flow).
- Vim-style keybinds: palette on `Ctrl+F`, `Ctrl+P`/`Ctrl+N` prev/next in the
  palette, `Ctrl+O` toggles the stack, `Ctrl+C` copies the stack (defers to the
  OS copy when text is selected), `Ctrl+Enter` sends.
- nvim-style CLI: `dreamd file.md` opens the file on load while the tree stays
  rooted at the current directory's repo.

### State

Compiles clean (`cargo build`); launches and passes startup smoke tests. The
send loop was verified end-to-end (a real stack landed as a formatted query).

### Not yet verified / known limits

- Full GUI interactions (traffic-light alignment, drag, ⋯ menu, Trash
  round-trip) checked only by launch smoke tests, not interactively.
- Highlight DOM re-wrap uses single-node text search; heavily formatted
  selections may read as stale.
- tmux `claude`-pane detection is heuristic (may run as `node`); pin
  `tmux_target` in config for reliability.
- No unit tests yet — `locate()`/`reanchor` are the obvious first targets.
- Fuzzy search covers paths only; content/`live_grep` is a v2 item.
- Placeholder app icon (blue square).
