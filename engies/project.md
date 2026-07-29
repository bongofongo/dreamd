# dreamd — State of the Project

*This page is the daily landing spot for everyone on the team. It's kept up to date
automatically and written so that you can be away for a week, read this in five
minutes, and know exactly where things stand. Last updated: 2026-07-29.*

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

**Status: v1 is built and works.** The repo went from an empty scaffold to a
working app in a single session on 2026-07-24. It compiles clean (`cargo build`),
launches, and the send loop has been verified end to end — a real stack landed in a
Claude Code pane as a formatted query.

What exists: the file tree, the fuzzy palette, the reading view, highlight mode,
the annotation modal, the stack panel with cherry-picking, the stale-highlight
margin rail, live reload that preserves your scroll position, an nvim-style CLI
(`dreamd file.md`), a per-file `⋯` menu (copy path / delete to Trash),
vim-flavored keybinds throughout, and a settings panel for changing those keybinds
and the colour scheme without leaving the app.

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

- **2026-07-27** — **Claude Code now runs inside dreamd.** Press `Ctrl+T` and a
  terminal opens along the bottom of the reading pane with an assistant already
  started in the repository you are reading — the same one that can see your
  highlights and work through your queue. It is a real terminal, so everything
  you would type in one works, and the key that opened it closes it again from
  either side. Closing only hides it: the conversation is still there when you
  bring it back, and it is put away automatically in distraction-free reading
  mode and never printed. Nothing about it costs anything until you press the
  key for the first time — a session that never opens a terminal doesn't load
  one. Two caveats worth stating plainly: the pane has been tested but not yet
  *looked at* by a person, and it assumes the `claude` command is installed the
  usual way. If it isn't, the pane says the process exited and offers a retry
  button rather than failing silently.
  The app's icon also changed size. It had been drawn to fill its square, which
  is right for a browser tab and wrong for the Dock, where it made dreamd look
  larger than every app beside it. It is now drawn to Apple's grid.

- **2026-07-27** — **Your marks now survive closing the app.** Until today,
  highlights, the questions attached to them and the queue they form all lived
  only in memory: quit dreamd and they were gone. That was a deliberate rule, and
  it stopped being the right one the moment an assistant could work through your
  queue — a conversation that spans a lunch break shouldn't lose the questions you
  asked before it. They are now written to a small file in your own settings
  folder, one per repository, readable only by your account. Nothing is written
  into the repository itself, and your markdown is still never touched; that part
  of the rule has not moved.
  Three details worth knowing. Saving is deferred by half a second and happens
  again on quit, so typing a question never waits on the disk and closing the app
  never loses the one you just typed. If two dreamd windows are open on the same
  repository, only the first writes — the second keeps its marks for as long as it
  runs and stays out of the way, rather than the two overwriting each other. And
  the file is treated as untrusted on the way back in: a hand-edited or copied one
  cannot point dreamd at a file outside your repository, and a corrupted one costs
  you the marks rather than the launch.
  There is also a new way to look at all this from a terminal: `dreamd marks path`
  prints the file, and `dreamd marks prune` says what it *would* tidy up before it
  tidies anything. A question you asked and nobody answered is never deleted, by
  any of it.
  Two things not yet done: nobody has quit and reopened the real window to watch
  the marks come back — everything underneath that is tested automatically — and a
  second window doesn't yet *show* that it isn't saving, it only says so in a log
  nobody reads.

- **2026-07-27** — **The assistant can now read your queue, and the window answers
  back.** This is the one the last few sessions were building toward. When you
  highlight a passage and attach a question to it, that question joins an ordered
  list — the order you asked in. Until today that list only travelled one way: you
  pressed send, and it went out. Now an assistant working in a terminal beside
  dreamd can ask for the list directly ("here is what I asked, in order"), work
  through the questions with its own tools, and mark each one answered as it goes.
  The count in the dreamd window ticks down as it happens. You don't touch the
  window; you just watch it.
  How the two halves find each other is worth a sentence, because it is the reason
  there is nothing to configure. dreamd opens a private channel named after the
  repository you're in — a file only your account can read, no network port, no
  password, nothing to expose. The assistant, started from anywhere inside the same
  repository, works out the same name and connects. Two dreamd windows on the same
  repository is handled too: the second notices the first already owns the channel
  and says so rather than fighting over it.
  Two deliberate limits. The assistant can read your marks and close them; it
  cannot reorder your list, delete a mark, or write to any of your files — editing
  stays where it was. And a closed question keeps its highlight: the evidence
  stays on the page, so "answered" doesn't mean "erased".
  One thing has *not* been checked yet: nobody has sat in front of the finished
  window and watched the count go 3, 2, 1, 0. Every part underneath it is tested
  automatically; the whole loop, with human eyes on it, is the next thing to do.

- **2026-07-27** — **Every highlight got a name that means something.** Groundwork,
  and worth explaining because of what it unblocks. Each highlight you make has an
  internal label so the rest of the program can refer to it. Those labels were
  counted off from one, starting over every time dreamd opened — so the third
  highlight of today and the third highlight of tomorrow had the *same* label. That
  costs nothing while everything lives and dies inside one run, which is how dreamd
  has always worked. It becomes a real problem the moment anything remembers a
  highlight after you quit, or an assistant writes one down and comes back to it
  later. Both of those are next on the roadmap. Labels are now unique for good.
  The same change settled the full shape of what a highlight *is* — including two
  pieces nothing reads yet: who made the mark (you, or an assistant) and whether a
  question about it has been answered. Deciding all of that once, up front, is
  deliberate: several strands of the next feature will be built against it at the
  same time, and a shape that shifts underneath them would be expensive.
  A separate check also joined the automatic ones: 611 stored examples that confirm
  a highlight still lands on the right words after the surrounding text is edited.
  That check existed but only ever ran by hand, and the code it guards had just
  moved.

- **2026-07-27** — **The project got a safety net. It had almost none.** Until today,
  nothing checked the code automatically. There was one piece of automation, and it only
  ran when a *release* was being cut — so a change could sit on the main branch for a week
  before anyone discovered it didn't compile. The command that normally runs a project's
  tests ran **zero** tests, because none had ever been written. Now: every change is
  checked automatically the moment it lands, and there are **99 tests** where there were
  none.
  **What the tests actually protect.** dreamd reads files it did not write. A markdown
  document is untrusted input — it can contain anything, including deliberate attempts to
  make the app do something it shouldn't. There were five rules protecting against that
  (a document can't run code inside the app window; it can't hand the operating system a
  dangerous kind of link; it can't reach outside the folder you opened, whether through a
  link, a picture, or a delete). Every one of those rules was real and working — and not
  one of them was checked by anything. If a future change broke one, nothing would have
  noticed. All five are now tested.
  **And the tests were tested.** A test that passes proves nothing on its own: it might
  pass because the rule works, or because the test never really checks anything. So each
  protection was deliberately broken, one at a time, to confirm its test went red — then
  put back. All five failed as they should have. Only then was the coverage claimed.
  **A change that was measured and then thrown away.** Part of the work was meant to speed
  up releases by not rebuilding a tool every time. The measurement said it made no
  difference at all — the tool was already being reused, and the slow run that started the
  whole idea was a one-off. So the change was removed, and the reason written down where
  the next person will read it. Being wrong is cheap when you measure; it is expensive
  when you don't.

- **(earlier)** — In-document search with `/`, `n`/`N` (2026-07-27); an
  overnight-written batch of reading features reviewed and fixed — contents
  panel, code-block copy button,
  print/PDF, vim-style bookmarks, back/forward navigation — after a first pass
  caught a bug that made distraction-free mode render a blank window
  (2026-07-27); unique highlight ids as groundwork for later persistence
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
