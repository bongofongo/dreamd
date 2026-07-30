# dreamd — State of the Project

*This page is the daily landing spot for everyone on the team. It's kept up to date
automatically and written so that you can be away for a week, read this in five
minutes, and know exactly where things stand. Last updated: 2026-07-30.*

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
- **`pty`** — the embedded terminal pane (press `Ctrl+T`) that runs Claude Code
  inside dreamd's own window, docked at the bottom or the right. It can now be
  told which **permission mode** to start Claude Code in — how much it's allowed
  to do without asking you first, from "ask before every edit" up to "don't ask
  at all" — from a dropdown in the pane's own header.
- **`mcp`** — the private channel (no network port, just a file only your account
  can read) that lets the Claude Code running in that pane ask dreamd for your
  highlight queue and mark items answered as it works through them.

Two ideas that come up constantly:

- **Re-anchoring.** When you edit a file that has highlights in it, dreamd tries to
  find each highlighted passage again in the new text. If it can, the highlight
  survives. If your edit changed *the highlighted words themselves*, it can't — so
  the highlight is marked **stale**, turns red, and moves to the margin with a `?`
  asking whether it's still relevant. Better to ask than to silently point at the
  wrong text.
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
without leaving the app.

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

- **2026-07-28** — **The automatic checks now start the program, which they had
  never done.** The entry below ends on the observation that "the checks pass" and
  "a person can install it and use it" are different questions. This session
  closed most of that gap. Until now every check stopped short of the one thing a
  user does first: none of them opened dreamd. A change could compile, pass all
  208 tests, and still fail to put a window on the screen — and nothing would have
  noticed until somebody downloaded it.

  Two things changed. Dreamd now survives starting up on Linux machines with
  NVIDIA graphics cards, where it previously died before its window appeared, with
  an error message that came from deep inside the graphics system and could not be
  caught or reported. And the checks now launch the program on every change, then
  take the finished downloads and *install and run them* on four different Linux
  distributions that did not build them — which is the closest an automatic check
  can get to being a user.

  Getting there took three rounds of failures, all in the checks rather than in
  dreamd itself, and one of them is worth recording: a packaging step failed with
  a one-line error that gave no reason, and the fix turned out to be teaching the
  packaging tool to explain itself rather than guessing at the cause. It then named
  the real problem immediately. Two things are still checked only by hand, and are
  written down as such rather than assumed: whether the all-in-one Linux download
  really carries everything it needs, and the NVIDIA startup fix itself, which
  needs hardware no automated machine has.

- **2026-07-27** — **The Linux version was run on a real Linux machine for the
  first time, and three things were wrong.** The entry below describes making
  Linux a supported platform; that work had been written and checked
  automatically, but never actually installed and used by a person. Doing so
  found problems that automatic checks are not shaped to catch, because they only
  appear once the program is packaged and put on a desktop:

  Right-clicking a markdown file and choosing "Open with" never offered dreamd —
  for a program whose entire job is reading markdown, that is the one place
  people would look for it. It now appears there, and opens the file you picked.
  It was also shipping its icon in the wrong sizes, so it appeared blurry in
  menus and app grids; the missing size has been added. And building the Linux
  download on an up-to-date Linux machine failed outright, with a one-line error
  that explained nothing — the cause was a stale tool buried inside the packaging
  software, and the workaround is now written down in the three places somebody
  hitting it would actually look.

  Everything the automatic checks cover passed on Linux unchanged, which is the
  reassuring half of the result. The useful half is that "the checks pass" and
  "a person can install it and use it" turned out to be different questions.

- **2026-07-27** — **dreamd runs on Linux now, not just on a Mac.** It always
  *could* in principle — the engine underneath was written to work on both — but
  in practice the program had stopped compiling there months ago and nobody
  noticed, because everything that checks the code automatically only ever ran on
  a Mac. One small piece of Mac-only code was being called from a place that
  wasn't Mac-only, and that was enough to break the whole build. That is fixed,
  and the fix that matters more is the reason it went unnoticed: **every automatic
  check now runs twice, once on each platform**, so the same thing cannot quietly
  happen again in either direction.

  What that gets you: releases now include a Linux download in three formats
  (a single self-contained file that runs anywhere, a standard Debian/Ubuntu
  package, and a recipe for Arch Linux), the automated checks can now be run by
  anyone on a Linux machine rather than only on the author's laptop, and Linux is
  a real place to develop dreamd rather than a place you can only read about it.
  There is also a new automated performance check that runs on both platforms and,
  more usefully, rehearses the *release build itself* on every change — a release
  is frozen once it's published, so discovering the Linux half is broken at that
  point is the expensive way to find out.

  Two honest caveats. The Linux side has been written and reasoned about
  carefully, but it has not yet been run by a person on an actual Linux machine —
  the automated checks are its first outing. And the menus differ between the two
  platforms on purpose: the Linux toolkit simply refuses to draw several of the
  menu entries macOS uses, so rather than ship a menu with empty sections, Linux
  gets a shorter one, and the "open a folder" shortcut moved there because the
  Mac shortcut would have silently stolen a key the app already uses.

- **(earlier)** — Claude Code embedded in a pane inside dreamd, `Ctrl+T` to open
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
