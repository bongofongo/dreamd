# dreamd — State of the Project

*This page is the daily landing spot for everyone on the team. It's kept up to date
automatically and written so that you can be away for a week, read this in five
minutes, and know exactly where things stand. Last updated: 2026-08-03.*

## What we're building

dreamd is a small desktop app for **reading** markdown files. Not editing them —
reading them. It opens in its own window, shows a file tree of every markdown file
in whatever code repository you're sitting in, and renders the one you click with
proper typography: real fonts, real spacing, syntax-highlighted code blocks.

The person it's built for works all day inside a terminal — tmux for windows,
Neovim for editing, Claude Code as their AI assistant. Terminal text is fine for
code and miserable for prose, and existing markdown previewers dump you into a
Firefox tab, which breaks the flow. dreamd is the reading window that belongs in
that setup.

The second half is the part that makes it more than a pretty viewer: the
**highlight → annotation → stack → send** loop.

1. You select a passage in the document and **highlight** it.
2. You attach an **annotation** to that highlight — a question or comment, e.g.
   *"why does this contradict the section above?"*
3. The highlight + annotation pair goes onto a **stack** — a shopping basket of
   things you want to ask about.
4. When the stack is ready you **send** it, in one action, to your AI agent as a
   single formatted question with all your evidence attached.

So instead of retyping quotes into a chat box one at a time, you read normally,
mark up as you go, and ship the whole batch at once.

Two deliberate constraints worth knowing:

- **Read-only.** Editing stays in Neovim, where it belongs. dreamd never writes to
  your markdown.
- **Nothing is saved inside your repository.** Highlights, annotations and the
  stack are written to a small file in your own settings folder — one per
  repository, readable only by you — so they are still there tomorrow. That is a
  change from v1, which kept them in memory and lost them on quit; it stopped
  being the right rule once an assistant could work through the queue across
  sessions. Your markdown, and everything else in the repository, is still never
  written to.

## How it's built

Tech stack: **Rust** for the backend, **Tauri 2** to make it a real desktop app,
and plain **HTML/CSS/JavaScript** for the interface — no frontend framework.

Tauri matters here. It uses the operating system's own web engine (WKWebView on
macOS, WebKitGTK on Linux) instead of bundling a whole copy of Chrome the way
Electron does. Result: the app is its own window, not a browser tab, and the binary
stays small.

The Rust side is seven small modules, each with one job:

- **`fs_walk`** — finds every markdown file in the repo and builds the tree. Uses
  the `ignore` crate (the same one ripgrep uses), so it respects `.gitignore` for
  free.
- **`markdown`** — turns markdown into HTML (`pulldown-cmark`) and colors the code
  blocks (`syntect`). Raw HTML inside your markdown is **escaped**, not executed —
  that's the security fix that closes an XSS hole. It also does `locate()`, which
  finds where a piece of text lives in a file; that one function powers both
  highlight anchoring and the `file:line` references in sent queries.
- **`annotations`** — holds the highlights, annotations, and stack in memory, and
  re-anchors highlights when a file changes on disk.
- **`marks_file`** — reads and writes that same set to your settings folder, and
  decides what a file coming back off disk is allowed to say.
- **`search`** — fuzzy search over file paths using `nucleo` (the matcher behind the
  Helix editor). This is the Telescope-style palette.
- **`send`** — assembles the stack into a temp file, finds a tmux pane running
  `claude`, and types a fixed `read @<file>` prompt into it. Your highlighted text
  never gets pasted into a shell command, so nothing you highlighted can be
  interpreted as a command. No tmux? It falls back to your clipboard.
- **`watcher`** — notices when files change on disk (because you're editing in
  Neovim in another pane) and tells the UI to refresh. Also hot-reloads the theme
  CSS when you save it.
- **`config`** — reads a global `~/.config/dreamd/config.toml`, overridden by a
  repo-local `.dreamd.toml`, and now writes it too when you change something in the
  settings panel.
- **`theme`** — the catalogue of colour schemes: the ten that ship inside the app
  (each with a light and a dark half), plus any you have saved yourself, and the
  logic that picks which half to show.
- **`cli`** — the `dreamd theme …` and `dreamd config …` commands you can run from
  a terminal without opening the window.
- **`agent`** — the pane (press `Ctrl+T`) where you talk to Claude, docked at
  the bottom or the right, and resizable by dragging its edge. Claude's replies
  are laid out here by dreamd itself, in the same typography and colours as the
  document, with a quiet list of what it did beneath them. When it wants to do
  something it hasn't been pre-approved for, a card appears in this pane with
  Allow / Always allow / Deny — and if nobody answers, the answer is no. The
  pane's header also chooses a **permission mode**: how much Claude may do
  before asking, from "ask before every edit" up to "don't ask at all". It can
  also **pop out** into a card centred on the window instead of living in the
  dock — useful for a quick answer you asked for in passing rather than a
  conversation you're working alongside; the same conversation moves between
  dock and card without losing anything mid-reply.
- **`pty`** — the older version of that pane, which embedded Claude Code's own
  terminal interface instead. Kept as a fallback (Settings → Window) for
  anything the pane above cannot draw yet, and due to be removed.
- **`mcp`** — the private channel (no network port, just a file only your account
  can read) that lets the Claude Code running in that pane ask dreamd for your
  highlight queue and mark items answered as it works through them. If Claude
  Code isn't registered to use it yet, the pane shows a **Register** button that
  runs the one-time setup command for you, rather than making you type it.

Two ideas that come up constantly:

- **Re-anchoring.** When you edit a file that has highlights in it, dreamd tries to
  find each highlighted passage again in the new text. If it can, the highlight
  survives. If your edit changed *the highlighted words themselves*, it can't — so
  the highlight is marked **stale**, turns red, and moves to the margin with a `?`
  asking whether it's still relevant. Better to ask than to silently point at the
  wrong text.
- **One highlight per passage.** You can't highlight text that is already
  highlighted. Before, you could — but only the newest mark was clickable, so the
  older ones underneath became questions you could no longer read, edit or
  delete. Dragging over an existing highlight now opens *that* one instead of
  making a second. Because of that, you can change your mind about where a
  passage starts or ends: press **Resize** (in the highlight's own menu, or the
  `⤢` on its card in the stack), drag the new extent, press Enter. The question
  attached to it and its place in the queue come along unchanged.
- **The whole app is themeable.** A theme is split in two: one stylesheet holds the
  *rules* (how big a heading is, how much space between paragraphs), and a **palette**
  is a short list of colours and font settings. Ten palettes ship with the app —
  the reading-first ones (dreamd, Manuscript, Letterpress, Athenaeum) and the
  programmer-coded ones (Gruvbox, Catppuccin, Tokyo Night, Nord, Solarized, a
  high-contrast pair) — and each carries **both a light and a dark half** in one
  file, so light/dark is a switch of its own rather than a different theme. You can
  write or edit your own from the settings panel or a text editor; save the file and
  the window restyles instantly. Reading comfort is the product, so the paint job is
  a first-class knob.

## Where things stand right now

**Status: v1 is built and works, and shipped as 0.2.0 on 2026-07-29.** The repo
went from an empty scaffold to a working app in a single session on 2026-07-24.
It compiles clean (`cargo build`),
launches, and the send loop has been verified end to end — a real stack landed in a
Claude Code pane as a formatted query.

What exists: the file tree, the fuzzy palette, the reading view, highlight mode,
the annotation modal, the stack panel with cherry-picking, the stale-highlight
margin rail, live reload that preserves your scroll position, an nvim-style CLI
(`dreamd file.md`), a per-file `⋯` menu (copy path / delete to Trash),
vim-flavored keybinds throughout, and a settings panel for changing those keybinds,
the colour scheme, and which bars your desktop draws around the window — all
without leaving the app. Keybindings now come in three "spellings" — `linux`
(the default, unchanged from before), `mac`, and `vim` — chosen once in
settings; the underlying key combos are stored the same way regardless, so
switching spelling or rebinding a key never rewrites anything but how it's
shown. `vim` mode adds `j`/`k`/`d`/`u` to scroll the document (a line, or half
a screen, at a time) and `Ctrl+H`/`Ctrl+J` to hop between panes, and scrolling
with any of those keys **glides** to its target rather than jumping, still
landing in the same place if you hold the key down or if a fast press and a
slow one both queue up.

Known limits, all deliberate for v1:

- Fuzzy search matches **file paths only** — searching inside file contents
  (`live_grep`) is a v2 item.
- Highlight anchoring matches on the selected text with whitespace normalized,
  plus the text either side of it. Heavily formatted inline selections may still
  fail to re-locate and will read as stale.
- Your reading session now persists — highlights, annotations and the stack are
  saved to your settings folder and come back when you reopen. Two limits: the
  saved line numbers are re-checked per file the first time you open it, so a
  document edited elsewhere corrects itself when you look at it rather than at
  launch; and if two windows are open on the same repository, only the first is
  saving.
- The settings panel rewrites the config file when you change something. The values
  you set by hand are kept; comments in the file are not.

## Measuring speed

Speed is the priority after the interface itself, and the project now measures it
rather than guessing. Everything runs on this machine — there is no build server
involved.

There are three depths, so the cost of checking matches the size of the change:

- **quick** (about a minute) — run right after an edit, to catch anything that got
  obviously slower.
- **pass** (about five minutes) — run before committing. Launches the real app and
  times the thing that matters most: saving a file in your editor and seeing the
  reading view catch up.
- **deep** (about fifteen minutes) — run when investigating. Builds the fully
  optimized app and records a profile showing exactly where the time goes.

Every run compares against a stored set of reference numbers (a *baseline*) and
prints only what moved. The baseline is only ever updated on purpose, as part of the
change that earned it — a reference point that quietly drifts would hide the slow
decline it exists to catch.

Two honesty rules are built into the reports. First, some measurements come from
Google's browser engine standing in for the real one, because it is far quicker to
script; those numbers are reliable for spotting *that* something got slower, and are
never presented as the app's true timings. Second, the tests run against a generated
set of documents — from small notes up to 2MB — that is identical every time, since
a measurement is only comparable if what it measured is.

The first run explained both symptoms that prompted the work, and the next session
fixed them.

Saving a file re-checks every highlight in it, and each check used to rebuild a full
index of the document from scratch — about 7 milliseconds per highlight on a 2MB
file. Worse, a single save was being noticed **more than once** (about 1.6 times), so
the document was redrawn repeatedly and the work piled up: with a hundred highlights,
saving took over five seconds. The index is now built once per save instead of once
per highlight, and repeated notifications for the same save are folded into one. That
particular check is roughly **six times faster**, and a save is now noticed exactly
once.

Opening a large document took about 1.4 seconds. Measuring it corrected an assumption
we had: the app did scan the folder twice before drawing, but that only accounted for
about 60 milliseconds. Almost all of the wait was converting the markdown into HTML —
specifically the syntax colouring of code blocks, which turned out to be nearly the
whole cost. Since each code block can be coloured independently, they are now done
across all the machine's cores at once, which cut that work by about **three
quarters**. The double folder scan is gone as well.

Two ideas were measured and then deliberately *not* adopted. Scanning the folder
across several cores is much faster on a large project but noticeably slower on a
small one, because starting the extra workers costs more than it saves there. And a
browser feature that skips drawing off-screen parts of a document made the first draw
dramatically cheaper but made scrolling more expensive — and scrolling is what
reading actually is. Both decisions are recorded with their numbers so they can be
revisited rather than rediscovered.

The measurements also turned up two bugs nobody had reported. If you highlight text
that crosses a formatting boundary — part bold, part link, part code — the highlight
usually fails to reattach and gets quietly marked stale, even though the selection was
perfectly valid; that one is still open. The second was that highlights could reattach
to the *wrong* place when the same wording appears more than once in a document. That
one has since been fixed — and turned out to be far more common than the first estimate
of one in fifteen. See the most recent update below.

None of this was known before it was measured, and two of our guesses about the
causes were wrong.

## Glossary (no shame in needing it)

- **tmux** — a terminal multiplexer: splits one terminal window into many panes,
  each running its own program. dreamd's "send" feature types into the pane where
  Claude Code is already running.
- **Tauri** — a framework that wraps a web-tech UI and a Rust backend into a native
  desktop app. Like Electron, but lighter, with Rust instead of Node.
- **Webview** — the operating system's built-in web page renderer, borrowed by the
  app to draw its interface.
- **XSS (cross-site scripting)** — when text that should just be *displayed* gets
  *executed* as code instead. Escaping raw HTML is what prevents it.
- **Anchoring** — remembering *where* in a document a highlight sits, well enough to
  find it again after the document changes.
- **Fuzzy search** — matching `srtl` to `src/tools/list.md`; you type fragments, it
  ranks the likely files.
- **Crate (Rust)** — a Rust package/library. `ignore`, `notify`, `syntect` are
  crates we depend on.
- **Benchmark** — a timed, repeatable test of one small piece of code, run many times
  so the average means something.
- **Baseline** — the stored "this is how fast it was" numbers that today's run is
  compared against.
- **Profiling** — recording where a program actually spends its time, function by
  function, instead of reasoning about where it probably does.
- **Regression** — something that used to be fast and now isn't.
- **Palette (theming)** — the short list of colours and font settings that make up
  one theme, kept separate from the rules that say how the page is laid out.
- **TOML** — the plain-text format the settings file is written in. Designed to be
  readable and editable by hand, unlike JSON or XML.
- **XDG** — the convention that says a program's settings belong in `~/.config`.
  macOS has its own idea about this; dreamd follows the convention instead, because
  that is where its users look.

## Recent updates

- **2026-08-03** — **The same passage can only be highlighted once, and any
  highlight can now be resized.** Highlighting text that was already highlighted
  used to work, and quietly cost you the older mark: only the topmost one
  answered a click, so the question attached to the one underneath became
  unreachable — still in the queue, still counted, impossible to open. Dragging
  over an existing highlight now opens that highlight rather than stacking a
  second one on top of it. That would have made a mistimed drag permanent, so the
  other half shipped with it: **Resize**, from the highlight's own menu or the
  `⤢` on its card in the stack panel. Drag the new extent, press Enter, and the
  question you attached and its position in the queue survive untouched — the
  stack button even opens the right file and scrolls to the passage first.
- **2026-08-03** — **Light and dark got a keyboard shortcut, and "System"
  stopped lying about what your machine wants.** `Ctrl+Shift+D` flips the
  appearance and remembers it, without opening anything — for the moment the
  light in the room changes rather than a settings decision. Going back to
  following the operating system stays in the settings panel, which is the only
  place that can tell you what "System" would actually give you. That label was
  also wrong until now: trying Light once made the button read "System (light)"
  and, worse, made *choosing* System from then on mean light for the rest of the
  session. A window that has an appearance pinned over it cannot see the
  machine's own preference, so dreamd now remembers that answer separately and
  re-asks it at the one moment it can. The agent pane also opens faster: two
  separate program start-ups that used to happen the first time you opened it —
  around a second and a half between them — now happen in the background at
  launch, or not at all.

- **2026-07-31** — **A settings-panel check broke quietly and stayed red
  through the 0.2.1 release.** Adding the keymap-mode picker (see the entry
  below) put a new row at the top of the shortcuts list that doesn't look
  like the others — it's a dropdown, not a rebindable key — but the automated
  check that walks every settings row was still looking for the *first* row
  to have a key button. It found none, timed out, and the whole check crashed
  right there: no summary got printed, so the 25 checks before that point and
  the 300 after it never showed up in the CI log at all, even though most of
  them were passing. Each row is now found by *which shortcut it is* rather
  than by its position in the list, and the row count is read from the
  settings panel's own list of shortcuts instead of a number someone has to
  remember to update by hand. This also started `docs/patch-log.md`, a short
  log of one-fix repairs like this one — separate from the full session log —
  so a fix like this leaves more behind than just a commit message.

- **2026-07-30** — **dreamd 0.2.1 released**, wrapping the keybinding and
  agent-pane work below.

- **2026-07-30** — **Keyboard shortcuts learned three spellings, and vim
  motions came with the third one.** Settings now has a keymap **mode** —
  `linux` (default, byte-for-byte how the app always behaved), `mac`, or
  `vim` — that changes how a shortcut is *shown and pressed*, not what it
  does: every binding is still stored the same way underneath, so switching
  mode or rebinding a key never needs to touch two places. In `vim` mode,
  `j`/`k`/`d`/`u` scroll the document (a line, or half a screen, measured
  from the theme's own line spacing) and `Ctrl+H`/`Ctrl+J` move focus between
  panes — both dispatched so they still work as ordinary letters inside a
  text box like the find bar.

  Scrolling with any of those keys now **glides** instead of jumping straight
  there, easing toward a moving target rather than restarting with every
  keypress — ten fast presses and ten slow ones land at exactly the same
  place. A separate bug this surfaced: `vim` mode was stripping the modifier
  off shortcuts even while you were typing in a text field, so a Ctrl+F search
  or a comma in the annotation box could accidentally open the command
  palette or settings instead of typing the character. Five shortcuts that
  make sense to reach *from* a field (palette, settings, the agent pane, and
  the two pane-switching keys) now keep their modifier there; everything else
  behaves like an ordinary keystroke while a field has focus.

- **2026-07-30** — **The agent pane learned to dock right, pop out as its own
  card, and be dragged to a different size — and to register itself with
  Claude Code.** The pane already drew Claude's replies natively (see the
  entry below); this batch of sessions made it a real panel rather than a
  fixed box. It can dock at the bottom or the **right** of the window, be
  resized by dragging its edge, and **pop out** into a card centred on the
  window for a quick question instead of a whole conversation — the same
  live conversation moves between dock and card without losing anything
  mid-reply, because it is one piece of the page being relocated rather than
  redrawn. If dreamd's MCP connection (the private channel that lets Claude
  read your highlight queue) was never set up, the pane now shows a
  **Register** button that runs the one setup command for you, rather than
  printing it and leaving you to type it into a terminal.

- **2026-07-30** — **The agent stopped being a terminal.** Until now, asking
  Claude a question inside dreamd meant a real terminal embedded in the window:
  Claude Code's own interface, with its own colours, its own typeface, and a
  text box dreamd could not see into. You read a beautifully typeset document,
  highlighted a sentence, asked a question about it — and the answer came back
  looking like a terminal.

  The conversation is now dreamd's. The reply is laid out with the same
  typography, the same colours and the same code highlighting as the document
  you asked about, because it goes through exactly the same machinery. Beneath
  the words there is a quiet one-line-per-step list of what Claude actually did
  — which files it read, which commands it ran, each ticked when it finished —
  so you can see the work without it filling the pane.

  The bigger change is **permission cards**. Before, if Claude wanted to do
  something it had not been pre-approved for, it asked in a terminal nobody was
  watching, and the request simply sat there looking like nothing had happened.
  That is why the old pane pre-approved a small fixed set of harmless things and
  could never safely offer more. Now the request appears as a card in the window
  you are already reading in, saying what Claude wants to do, with Allow, Always
  allow, and Deny. "Always" lasts for that conversation only and is never written
  to your settings.

  Everything about this refuses in the safe direction: if the window is closed,
  if you never answer, if anything at all goes wrong, the answer is no. There is
  no way for it to accidentally say yes.

  Two smaller things came with it. **Escape now stops Claude mid-answer** — the
  old terminal claimed that key, so stopping a reply meant Ctrl+C. And the
  terminal pane is still there as a fallback, in Settings → Window, for anything
  the new one cannot draw yet; it will be removed once nobody needs it.

  **Not yet checked by a human:** the new pane has been verified by automated
  tests down to what the page *knows*, but nobody has yet looked at it in a real
  window. How it actually looks is still an open question.

- **2026-07-30** — **dreamd 0.2.0 released.** The first release since 0.1.0
  (2026-07-26) — 91 commits behind it, and a minor version bump rather than a
  patch because almost none of 0.2.0 existed at 0.1.0: the embedded Claude Code
  pane, the MCP server and its five tools, marks persisted to
  `~/.config/dreamd`, and the window-chrome toggles are all new since then.
  Everything in the release, including the window-chrome toggles and the four
  reading fixes below, had already landed and been verified the day before —
  this commit only bumped the version number.

- **2026-07-29** — **Four things that got in the way of reading, fixed.** All four
  came from using dreamd rather than from testing it.

  The **bottom of the window** was swallowing things. A file's `⋯` menu opened
  downwards with no check that there was room, so for any file in the lower half of
  the tree the menu appeared below the edge of the screen — invisible, though it
  was open. It now flips above the button when it has to. Separately, the Claude
  Code pane was losing its last row, which is where Claude's input box is drawn:
  the terminal was being told it had more room than it did, and the pane quietly
  cut off the difference. Both are exact now.

  A highlight no longer says **"? still pertinent"** unless its text really
  changed. It used to raise that on files nobody had touched — dreamd looks for the
  highlighted words in the file, and if you had highlighted across something bold
  or a link it could never find them, so it assumed the worst every time you
  reopened the file. It now only asks the question about a passage it once found
  and can no longer find. That margin strip is meant to be empty almost always,
  and now it is.

  **Sending a question is the end of it.** Every passage you sent used to put a
  card in that margin strip saying "with the agent", with an "Answered" button
  whose only job was to agree that yes, it was dealt with. Five questions meant
  five cards over the paragraph they were about. The cards are gone; a question you
  have asked is assumed handled.

  And **highlights that span bold text, links or code now stay highlighted.** They
  used to appear when you made them and then disappear the next time the page
  redrew — still counted, still in your stack, just invisible. The highlight is
  drawn in pieces now, so a phrase with a bold word in the middle gets one
  continuous wash and keeps its bold. Passages you have sent or taken off the stack
  also fade to the "done with" shade straight away, rather than waiting for the
  next time you open dreamd.

- **2026-07-29** — **Two bars of clutter removed from the Linux window, both on
  a switch.** Opening dreamd on Linux used to cost you two rows of furniture
  before the first line of prose: a File / Edit / Help menu bar, and directly
  above it the window's own bar with the close, minimize and maximize buttons.
  Both are now gone by default, so the window starts at dreamd's own toolbar and
  the document gets the space.

  Neither is a decision made for you. A new **Window** tab in Settings has a
  switch for each, and flipping one takes effect straight away — no restart. If
  you liked the menu bar, turn it back on and it comes back. One thing to know:
  the menu bar's two "open a folder" shortcuts belong to the bar, so they go away
  with it. Nothing is lost — click the folder name above the file tree and type a
  path, and it will complete as you go.

  macOS is untouched. Its menu bar belongs to the application rather than to the
  window, and its title bar is already drawn *inside* the reading area, so there
  was never a second bar there to reclaim.

  One deliberate limit: a project you have cloned cannot set either of these,
  even though it can still set your theme or the width of the file tree. A repo
  you have not read yet does not get to take the close button off your window.

- **2026-07-29** — **The send got instant, and the pane grew a header worth
  reading.** Sending your stack of questions to Claude used to wait five seconds
  before anything happened. The idea was that you could take it back in that
  window — but you almost never wanted to, and you paid the five seconds every
  single time. That delay is gone: pressing Ctrl+Enter now sends immediately.
  It no longer waits for Claude to look idle either, because Claude Code already
  queues a message typed while it is working. The one wait left is a pane that
  is still starting up, where typing too early would lose your question
  entirely; the bar on screen says so while it waits, and offers "Send now" if
  you would rather not.

  The button in the top-right corner used to be a clipboard icon that actually
  sent things. Now the clipboard icon copies to your clipboard, and a new
  paper-plane icon beside it does the sending.

  The pane's header gained two things. **Three model buttons** — opus, sonnet,
  haiku — that switch model mid-conversation without restarting anything, so you
  keep everything said so far. And a **status line that appears only when
  something is wrong**: if Claude cannot reach dreamd, it says so and tells you
  the one command that fixes it. Before this, that failure was invisible — Claude
  would answer your questions perfectly well and simply never tick them off,
  which looked like forgetfulness rather than a missing connection.

  Finally, Claude no longer asks permission to read your stack or the file you
  are reading. You highlighted the passage and typed the question, so the
  permission was already given — and the prompt was landing in a terminal you
  might not be looking at. Anything that *writes* still asks, exactly as before.

- **2026-07-28 (later the same day)** — **The Claude Code pane stopped being a
  plain terminal and started becoming the actual place the highlight →
  annotation → stack → send loop ends.** This was the first session against a
  new plan for that (`docs/plans/agent-ui-implementation.md`), and five of its
  seven pieces landed.

  The pane can now dock on the **right** side of the window instead of only the
  bottom, chosen from a dropdown in its own restyled header, alongside a
  **permission-mode** selector — how much Claude Code is allowed to do without
  asking you first, from "ask before every edit" up to "never ask". Changing it
  warns you that it restarts the conversation, and writes the choice to disk
  *before* restarting, so the pane and the file on disk can never disagree.
  Escape now closes the pane from inside the terminal, which costs Claude
  Code's own use of Escape (interrupting it) — a deliberate trade, written down
  as such rather than discovered later.

  Underneath that, a highlight now remembers two more things: whether a
  question about it is **sitting with the agent, unanswered** (distinct from
  "never sent" — the earlier state), and whether it was made in an **earlier
  session** rather than this one. Prior-session highlights now fade to a duller
  version of the theme's own highlight colour, so a page you're returning to
  visually separates what you're still deciding about from what you already
  knew. A hand-edited or copied marks file can't fake either flag — only
  actually being loaded by dreamd sets them.

  The reading window also picked up some overdue chrome: the file tree's width
  can be dragged instead of being fixed, a floating outline of the open
  document's headings can be popped open and dismissed with a click or a
  scroll, and the file-tree header is now an editable path field — type a
  folder, press Tab to autocomplete, and dreamd switches to it.

  The important half of what's *not* done yet: **Ctrl+Enter still doesn't talk
  to this pane.** Sending the stack still goes out over the tmux path described
  below. Making Ctrl+Enter open the pane, assemble the prompt itself, and watch
  for Claude Code going idle to know when to actually submit is the sixth
  planned piece, saved for a session of its own because it's judged the part
  most likely to need someone's full attention.

- **(earlier)** — the automatic checks started actually launching the program
  instead of just compiling and testing it, catching an NVIDIA/Linux startup
  crash nothing else could (2026-07-28); the Linux build run on a real Linux
  machine for the first time, fixing "Open With" never offering dreamd, a
  blurry icon, and a packaging failure caused by a stale tool (2026-07-27);
  dreamd made to run on Linux at all, with every automatic check now running
  on both platforms so a Mac-only regression can't hide there again
  (2026-07-27); Claude Code embedded in a pane inside dreamd, `Ctrl+T` to open
  it (2026-07-27); highlights, annotations and the stack persisted to
  `~/.config/dreamd` so they survive quitting, plus `dreamd marks path`/`prune`
  (2026-07-27); an assistant able to read the highlight queue over a private
  per-repo socket and mark items resolved as it works through them
  (2026-07-27); unique, permanent highlight ids as groundwork for that
  persistence (2026-07-27); the project's first safety net — CI on every push
  and 99 tests where there had been zero, covering the five security rules
  (escaping, link/scheme allowlists, repo-root containment) (2026-07-27);
  in-document search with `/`, `n`/`N` (2026-07-27); an
  overnight-written batch of reading features reviewed and fixed — contents
  panel, code-block copy button,
  print/PDF, vim-style bookmarks, back/forward navigation — after a first pass
  caught a bug that made distraction-free mode render a blank window
  (2026-07-27); dreamd 0.1.0 released, signed and notarized, installable via
  Homebrew or a one-line terminal command, after discovering unsigned macOS
  builds are reported as "damaged" (2026-07-26); an Apache 2.0 licence
  (2026-07-25); ten colour themes, each with a light and dark half, replacing
  the single dark default (2026-07-25); a settings panel for keybindings and
  themes — the first thing dreamd ever wrote to disk, and only ever
  preferences (2026-07-25); the highlight-reanchoring bug that misplaced about
  one highlight in three when the same wording appeared twice in a document,
  fixed and pinned by a 611-fixture regression test (2026-07-25); the first
  round of performance work, six changes with 53 numbers improved and none
  worse (2026-07-25); opening a single file no longer scanning the whole
  project first (2026-07-25); the public site at fongo.uk/dreamd (2026-07-25);
  a Rust cleanup pass and the three-tier perf measurement setup; and **v1
  shipped** — empty scaffold to a working app in one session, with the
  highlight → annotation → stack → send loop, an XSS fix, and a
  security-restricted external-link policy (2026-07-24).
