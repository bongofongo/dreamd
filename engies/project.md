# dreamd — State of the Project

*This page is the daily landing spot for everyone on the team. It's kept up to date
automatically and written so that you can be away for a week, read this in five
minutes, and know exactly where things stand. Last updated: 2026-07-25.*

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
- **Nothing is saved.** Highlights, annotations, and the stack live in memory and
  vanish when you close the app. This is on purpose for v1 — it keeps the whole
  thing simple and honest about what it is: a working surface for one reading
  session, not a note-taking database.

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
- Nothing from your *reading session* persists — highlights, annotations and the
  stack are gone when you close the window, by design. Your *preferences* do
  persist, and are the only thing the app ever writes.
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

- **2026-07-25** — dreamd became something you can install rather than something you
  have to build. Until now, using it meant cloning the repository and compiling it
  yourself, which rules out anyone who doesn't already have a Rust toolchain. There is
  now a proper Mac app: one command in Homebrew — or a single line pasted into a
  terminal — puts **dreamd in your Applications folder and the `dreamd` command on your
  path at the same time**. They're the same program, so the window and the command line
  can never end up as different versions of each other. Releases are built, signed and
  stamped by Apple automatically whenever a version is tagged, and the whole pipeline is
  written so that adding Linux later is a one-line change rather than a rewrite.
  Two things came out of this worth knowing. First, **the app was 43% app icon** — a
  quirk of the build tooling meant a 4 MB uncompressed picture was baked into the
  program, on a platform that never displays it. Removing that and trimming some unused
  libraries took the app from 9.8 MB to 5.6 MB, a little under half. Second, and more
  seriously: **double-clicking the app would have hung the machine.** Launched from the
  Finder rather than a terminal, dreamd had no idea which project you meant, and its
  answer was to start reading *every file on the disk* — before it had even drawn a
  window. It now opens quietly instead, waiting in the dock with a **File ▸ Open
  Folder** menu until you tell it what to read. That bug was invisible from the
  terminal, and would have been the first thing every new user hit.
- **2026-07-25** — dreamd learned to look like a book, and to follow your Mac's
  light/dark setting. Until now it shipped one dark, programmer-toned look plus nine
  colour swaps of it, and every one of them used the same sans-serif font — fine for
  a code editor, less good for something whose whole job is reading. There are now
  **ten themes, each with a matching light and dark half**, and the appearance is a
  separate switch from the theme: pick the look you like, then pick light, dark, or
  *system* (the default, which follows macOS and keeps following it if your Mac
  switches at sunset). Three of the themes are new and deliberately bookish —
  *Manuscript* (warm sepia paper, or vellum by candlelight), *Letterpress* (crisp
  black ink on cotton, justified like a printed page), and *Athenaeum* (a library
  after hours: brass on green-black). The **default theme was redesigned** around
  reading too: a serif face, a narrower column set to a comfortable line length, and
  a palette that's paper-with-a-hint-of-lilac by day and deep indigo by night. Every
  one of the new colour sets was checked against accessibility contrast standards
  rather than eyeballed; the themes borrowed from elsewhere (Nord, Solarized,
  Gruvbox…) keep their published colours even where those fall slightly short, since
  quietly "fixing" someone else's palette would make it no longer theirs. Old theme
  names still work, and old custom themes people wrote themselves keep working
  untouched. One caveat worth stating plainly: this was all built and checked in a
  test browser, because the tool that runs the real app isn't installed on the
  machine it was written on — **nobody has yet seen these themes in the actual app
  window**, and that's the first thing to do next.

- **2026-07-25** — dreamd has a licence: **Apache 2.0**. Until now the project had
  no licence file at all, which in law means all rights reserved — anyone reading
  the code on GitHub was technically not allowed to use it. Apache 2.0 is the
  permissive, business-safe choice: use it, change it, ship it, keep the copyright
  notice, and it grants patent rights alongside the copyright ones. The licence text
  now sits at the root of the repository, the program declares it, and the public
  page says so and links to it — that page had been carefully avoiding the word
  "licence" precisely because there wasn't one. The app also stopped wearing a
  placeholder: its icon is now the site's mark — a dark rounded square with the
  yellow highlight bar over two lines of text, the same image the browser tab shows.

- **2026-07-25** — Opening a single file got out of its own way. Running
  `dreamd notes.md` is meant to be the same gesture as double-clicking a document:
  you asked for one file, you should get one file. What actually happened was that
  the app first went and catalogued every markdown file in the surrounding project —
  and on a large project that is thousands of files — *before it would even put a
  window on screen*. You waited for a file list you hadn't asked for.

  That cataloguing now happens quietly in the background. The document opens
  straight away, and the file browser down the side starts closed, since you didn't
  ask for it; the button to open it is right there, and by the time you press it the
  list has usually finished building. If you do ask for it early — by opening the
  file search — it simply waits the moment it needs rather than showing you a wrong
  or empty answer. Launching on a folder instead of a file behaves exactly as before,
  browser open and ready.

  On a 5,000-file project, roughly 95 milliseconds of work now happens after the
  window exists instead of before it. That is the honest measurement; the
  end-to-end "time until you can read it" number moved too, but by less than the
  measurement's own noise, so it isn't being claimed.

- **2026-07-25** — Settings and themes. Until now, changing a keybinding or a colour
  meant editing a configuration file by hand and restarting; there was exactly one
  colour scheme, and it was the one baked into the app. Now `Ctrl+,` opens a settings
  panel with three tabs: click a shortcut and press the new keys you want; browse ten
  colour schemes that ship with the app — Gruvbox, Catppuccin, Tokyo Night, Nord,
  Solarized and a high-contrast light/dark pair — clicking to preview and applying to
  keep; or build your own with a colour picker per setting and save it under a name.
  Everything the panel does is also a terminal command (`dreamd theme set nord`,
  `dreamd config set keymap.palette Ctrl+Space`), because the person this is for
  lives in a terminal.

  Three things worth calling out. First, a deliberate rule change: the app used to
  write **nothing** to disk. It now writes your preferences, and only those, to a
  folder of its own — never to your files and never inside your project. Second, a
  bug that had been sitting there: dropping a small settings file into a project to
  change one thing silently reset *all* your keyboard shortcuts while you were in
  that project, because the code could not tell "this file says use the default"
  apart from "this file didn't mention it". Settings are now merged setting by
  setting, and that is checked automatically. Third, a security tightening — a
  settings file that comes with a project you cloned can now choose a colour scheme
  by name but can no longer point the app at an arbitrary file on your disk, which
  it could previously read and, through a stylesheet trick, send somewhere.

  Code blocks now recolour with the theme too. Their colours are worked out in
  advance rather than by the stylesheet, so a light theme used to leave you with
  dark code blocks; each palette now names the code colouring it wants.

  Three new automated checks came with it: one over the settings merging and saving,
  one that every shipped colour scheme is complete and valid, and one that drives the
  settings panel in a real browser engine and checks it behaves. The last one earned
  its keep immediately — it caught the theme editor coming up empty, because the code
  reading the colours out of a stylesheet was accidentally reading an *example* in
  that file's own comment.

- **2026-07-25** — Fixed the white flash on startup. Opening the app showed a blank
  white page for as long as the start-up took, then snapped to the proper dark
  background. The colours live in a stylesheet the app loads a moment *after* the
  window appears, so until it arrived there was no background to paint. The window
  is now given its colour up front, read from whichever theme file you actually
  use — so if you have written yourself a light theme, it starts light rather than
  flashing dark at you.

- **2026-07-25** — Fixed the highlight-in-the-wrong-place bug found the same day, and
  found it was much bigger than reported. A highlight remembers the words you selected;
  when the file is saved, the app looks those words up again to find out which lines
  they are on now. If the same wording appears twice in a document, it was picking
  whichever copy came first. The first estimate was that this affected about one
  selection in fifteen; the estimate turned out to be measured against a yardstick that
  was itself wrong, and the real figure was closer to **one in three**. Three things
  changed. The app now sends the text on either side of your selection along with the
  selection itself, which tells the copies apart. When a file is saved, it also uses
  where the highlight was a moment ago, which settles the cases where two copies are
  word-for-word identical including their surroundings. And the whole thing is now
  checked automatically: a new test runs all 611 sample highlights and fails if any
  lands in the wrong place. It passes with zero wrong. This is the project's first real
  automated correctness check — until now, correctness was verified by using the app.

- **2026-07-25** — The first round of speed work, using the measuring setup built the
  day before. Six changes: saving a file now checks all its highlights against one
  shared index instead of rebuilding that index for every single highlight; code
  blocks are syntax-coloured across all the machine's cores at once instead of one
  after another; repeated notifications for a single save are folded into one; the
  folder is scanned once at startup rather than twice; and the two slowest pieces of
  the reading view — reattaching highlights and moving through the file finder — were
  rewritten to stop re-reading the whole document each time. Measured end to end
  afterwards: **53 numbers improved and none got worse**. The largest single change
  made re-checking a hundred highlights about six times faster; colouring a large
  code-heavy document is about four times faster. Two further ideas were tested and
  rejected on the numbers rather than adopted on instinct. Also found, and left for a
  focused fix, a real bug: about one highlight in fifteen reattaches to the wrong copy
  of repeated wording.

- **2026-07-25** — dreamd has a public page for the first time, at
  **fongo.uk/dreamd**. One dark page, no screenshots: it opens on a starfield with
  the name floating on it, and the solid half of the site scrolls up over that like
  a curtain being drawn, so it reads as two pages joined by a scroll. Below the fold
  it explains what the app is, walks through the highlight → annotation → stack →
  send loop, and lists what it's built from. The page lives in a new `website/`
  folder inside this repo but is entirely separate from the app — it ships on its
  own, and nothing about it can affect or slow down the program. Two things it
  deliberately did not say at the time: it made no licence claim, because there was
  no licence file yet (there is now — see the entry above), and it never explains
  what "dreamd" means. Pictures, video,
  or a gallery tab can be added later without rearranging anything.

- **2026-07-24** — A cleanup pass over the Rust code: about thirty lines removed by
  deleting things that had been written twice (two copies of the folder scanner,
  three copies of the "is this a markdown file?" check, two copies of the
  stack-assembly code). Nothing about the app behaves differently — this was
  checked by rendering real documents through both the old and new code and
  confirming the output matched byte for byte. Two side effects worth knowing: the
  file palette got roughly 20% faster, and a lurking bug was removed where two
  files with the same path relative to the repo root could make one of them
  unreachable from search.

- **2026-07-24** — Built a performance measurement setup with three depths (about
  one minute, five minutes, fifteen minutes) so speed can be checked at a cost that
  matches the size of the change. It explained both slowdowns we'd been feeling —
  saves are noticed more than once and re-check every highlight from scratch;
  opening a large file is dominated by converting it to HTML, not by scanning the
  folder as we'd assumed. It also surfaced a bug: highlights spanning bold, links
  or code usually fail to reattach and are wrongly marked stale. No fixes yet —
  this round was about being able to prove them, and two of our guesses about the
  causes turned out to be wrong.

- **2026-07-24** — Hovering any icon-only button now pops up its name and keybind,
  so the toolbar is learnable without reading the docs. The keybind shown is read
  from your live keymap, so a custom binding displays correctly.
- **2026-07-24** — Added the `engies/` docs directory (this file), a `/wrap-up`
  skill that commits a session and logs it to `docs/session-log.md`, and a
  `/update-project-doc` skill plus a daily scheduled job that keeps this page
  current.
- **2026-07-24** — **v1 shipped.** Empty scaffold → working app in one session:
  backend modules (`fs_walk`, `markdown`, `annotations`, `search`, `send`,
  `watcher`, `config`), the full frontend, then a round of UI iteration (scrolling
  fix, highlight mode, collapsible panes, overlay titlebar, per-file menu, vim
  keybinds, nvim-style CLI). Security pass closed a raw-HTML XSS hole and
  restricted external-link opening to `http`/`https`/`mailto`. Full detail in
  `docs/session-log.md`.
- **2026-07-24** — Plan review reworked the original design: annotations were
  promoted from v2 to v1 core, and the send path was redesigned around a temp file
  instead of shell interpolation.
- **(earlier)** — Repo created with the scaffold and `docs/plan.md`.
