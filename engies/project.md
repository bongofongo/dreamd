# dreamd — State of the Project

*This page is the daily landing spot for everyone on the team. It's kept up to date
automatically and written so that you can be away for a week, read this in five
minutes, and know exactly where things stand. Last updated: 2026-08-05.*

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

**Status: v1 is built and works, and shipped as 0.2.0 on 2026-07-29** (followed by
0.2.1 on 2026-07-30 and 0.2.2 on 2026-08-03). The repo
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

- **2026-08-05** — **A nightly job now sweeps its own codebase.** A scheduled job
  runs once a night against one of fifteen areas of the repo, on a rotation so
  each area gets a look roughly every two weeks — always whichever has gone
  longest without one. Each sweep makes three passes: check whether this very
  document's claims about that area are still true against the actual code,
  look for anything worth simplifying, then fix what it found and tighten the
  relevant section. The work lands as a pull request on its own branch rather
  than straight to `main` — the one exception to the project's usual
  straight-to-main habit, because nobody is watching it happen live — except
  for the parts of the interface too large for the automated checks to prove
  correct, which get written up as proposals for a human to apply by hand
  instead of turned directly into code. A quiet night where nothing needed
  fixing counts as a success, not a failure to find something to change. The
  first run swept the project's own documentation and found three
  inaccuracies — a keybind description, a wrong default setting, and a stale
  description of how a sent question actually reaches Claude — now sitting in
  an open pull request.
- **2026-08-04** — **dreamd opens in front of you now, and the window's top edge
  was rebuilt.** Launching from the terminal used to put the window *behind* the
  terminal you launched it from — it was there, just not in front — while opening
  it from Spotlight worked fine. Fixed. On a Mac, the file tree now runs the full
  height of the window and holds the close/minimise/zoom buttons in its top-left
  corner while it is open; collapse the tree and they hand back to the bar. And
  the bar itself can dissolve rather than end at a line: text fades out as it
  scrolls up underneath it, the way Preview and the Claude desktop app do. That
  is on by default on macOS and can be turned off in Settings → Window. The
  setting that used to sit there — a "native titlebar" switch — is gone from the
  Mac, because it did nothing good: turning it off and on again left the window
  with an opaque grey bar stuck on top of the page and no way to get rid of it
  short of hand-editing a config file. The window buttons now simply stay.
- **2026-08-03** — **dreamd 0.2.2 released**, wrapping the highlight-overlap/resize
  work and the two agent-pane fixes below.
- **2026-08-03** — **Two small but visible agent-pane bugs fixed.** Clicking a
  model chip (opus/sonnet/haiku) used to take two clicks — the first press
  changed nothing on screen, and only the second (or whatever you typed next)
  made it visibly switch. Claude Code reports which model it's using at the
  *start* of a turn, before it has read what you just sent, so the chip was
  always confirming the *previous* turn's choice one step late. It now lights
  the moment you click it, and only reverts if the model genuinely hasn't
  changed within two turns. Separately, the agent pane's text box, its
  permission-card buttons and the embedded terminal were always a near-white
  colour no matter which theme was active, because they read a colour
  variable (`--fg`) that no theme file has ever actually defined — so every
  one of them silently fell back to the same hardcoded shade, most visible on
  light themes. They now follow the active palette like the rest of the
  window.
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

- **(earlier)** — the agent pane learned to dock right, pop out into a card,
  resize by drag, and show a Register button for the MCP connection
  (2026-07-30); the agent pane stopped being a plain embedded terminal and
  became dreamd's own typeset conversation view, with permission cards
  replacing an invisible terminal prompt (2026-07-30); dreamd 0.2.0 released
  (2026-07-30); four reading papercuts fixed — a menu that could open off
  the bottom of the screen, a clipped last row in the old terminal pane, a
  stale-highlight warning that fired too often, and highlights spanning bold
  text or links disappearing on redraw (2026-07-29); the Linux menu bar and
  window-frame bar made optional, off by default (2026-07-29); sending a
  stack made instant instead of a five-second wait, plus model-switch buttons
  and a connection-status line in the pane header (2026-07-29); the first
  pass at replacing the terminal pane with a native one, including a
  permission-mode selector and per-highlight "sent" state (2026-07-28); the
  automatic checks made to actually launch the program, catching an
  NVIDIA/Linux startup crash nothing else could (2026-07-28); dreamd made to
  run on Linux at all, with checks running on both platforms from then on
  (2026-07-27); Claude Code embedded in a pane for the first time, `Ctrl+T`
  to open it (2026-07-27); highlights, annotations and the stack persisted to
  `~/.config/dreamd`, plus an assistant able to read and resolve them over a
  private per-repo socket (2026-07-27); the project's first safety net — CI
  and 99 tests covering the security rules (2026-07-27); in-document search,
  a reviewed batch of reading features (contents panel, code-block copy,
  print/PDF, bookmarks, back/forward) (2026-07-27); dreamd 0.1.0 released,
  signed and notarized (2026-07-26); an Apache 2.0 licence, ten colour
  themes, and a settings panel — the first thing dreamd ever wrote to disk
  (2026-07-25); a highlight-reanchoring bug fixed and pinned by a 611-fixture
  regression test, and the first round of performance work (2026-07-25); the
  public site at fongo.uk/dreamd (2026-07-25); and **v1 shipped** — empty
  scaffold to a working app in one session, with the highlight → annotation →
  stack → send loop, an XSS fix, and a restricted external-link policy
  (2026-07-24).
