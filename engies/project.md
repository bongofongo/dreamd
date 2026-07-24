# dreamd — State of the Project

*This page is the daily landing spot for everyone on the team. It's kept up to date
automatically and written so that you can be away for a week, read this in five
minutes, and know exactly where things stand. Last updated: 2026-07-24.*

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

## Recent updates

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
