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
  repo-local `.dreamd.toml`.

Two ideas that come up constantly:

- **Re-anchoring.** When you edit a file that has highlights in it, dreamd tries to
  find each highlighted passage again in the new text. If it can, the highlight
  survives. If your edit changed *the highlighted words themselves*, it can't — so
  the highlight is marked **stale**, turns red, and moves to the margin with a `?`
  asking whether it's still relevant. Better to ask than to silently point at the
  wrong text.
- **The whole app is themeable.** `ui/theme.css` is a normal stylesheet. Point the
  config at your own copy, edit it, save — the window reloads instantly. Reading
  comfort is the product, so the paint job is a first-class knob.

## Where things stand right now

**Status: v1 is built and works.** The repo went from an empty scaffold to a
working app in a single session on 2026-07-24. It compiles clean (`cargo build`),
launches, and the send loop has been verified end to end — a real stack landed in a
Claude Code pane as a formatted query.

What exists: the file tree, the fuzzy palette, the reading view, highlight mode,
the annotation modal, the stack panel with cherry-picking, the stale-highlight
margin rail, live reload that preserves your scroll position, an nvim-style CLI
(`dreamd file.md`), a per-file `⋯` menu (copy path / delete to Trash), and
vim-flavored keybinds throughout.

Known limits, all deliberate for v1:

- Fuzzy search matches **file paths only** — searching inside file contents
  (`live_grep`) is a v2 item.
- Highlight anchoring matches on the selected text with whitespace normalized.
  Heavily formatted inline selections may fail to re-locate and will read as stale.
- Nothing persists, by design.

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
perfectly valid. And separately, about one highlight in fifteen reattaches to the
*wrong* place: if the same wording appears more than once in a document, the app
currently picks the first copy rather than the one you actually selected. Both are
still open.

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

## Recent updates

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
  deliberately does not say: it makes no licence claim, because the project still
  has no licence file, and it never explains what "dreamd" means. Pictures, video,
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
