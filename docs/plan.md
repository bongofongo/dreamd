# dreamd — lightweight markdown viewer/explorer for tmux+nvim+Claude Code workflow

## Context

Existing markdown-preview tools (e.g. neovim markdown-preview plugins) render in a real web browser (Firefox/Chrome), which doesn't fit a tmux + tty + Neovim workflow and can't be customized as an app in its own right. The user wants a standalone, cross-platform (macOS + Linux), Rust-backed markdown viewer that:

- Opens as its own application window (never a browser tab)
- Has a lightweight binary and minimal runtime overhead
- Offers a file-tree explorer plus fuzzy ("Telescope"-like) search over markdown files in the current repo
- Is themeable to a CSS/HTML level of control
- Can send a selected snippet of markdown straight to the user's already-running Claude Code session (their primary source of the markdown they read)
- Is read-only for v1 — no in-app editing (editing stays in Neovim), and no annotation layer (deferred to v2)

The `dreamd` working directory is currently empty — this is a from-scratch project.

## Decisions locked in with the user

1. **UI stack: Tauri.** Rust backend + OS webview (WKWebView on macOS, WebKitGTK on Linux) for the frontend. This is not a browser — it's the app's own window — and avoids Electron's bundled-Chromium weight, keeping the binary small. Frontend is real HTML/CSS/JS, giving full stylesheet-level theming with trivial live-reload.
2. **Explorer: file tree + fuzzy finder.** A collapsible directory tree of markdown files, plus a Telescope-style fuzzy-search overlay (keybind-triggered) over all markdown files in the repo. No link-graph/topograph view for v1.
3. **MVP scope: read-only viewer + "send to Claude."** No inline editing. Selecting text in the rendered preview and invoking a keybind/context menu sends that snippet (plus file path context) to the user's running Claude Code session. No annotation layer yet.

## Architecture

### Backend (Rust, `src-tauri/`)

- **File discovery** — `ignore` crate (the same crate ripgrep uses) walks the repo root for `*.md`/`*.markdown` files, automatically respecting `.gitignore`/`.ignore`. Produces a tree structure sent to the frontend.
- **File watching** — `notify` crate watches the repo for file adds/removes/edits (since the user edits in Neovim in another pane) and emits Tauri events (`file-added`, `file-changed`, `file-removed`) so the tree and open preview stay live. The same watcher also watches the active theme CSS file for hot-reload.
- **Markdown rendering** — `pulldown-cmark` (CommonMark + GFM extensions: tables, strikethrough, task lists, footnotes) converts markdown to an HTML string server-side. Fenced code blocks are syntax-highlighted server-side via `syntect`, producing highlighted `<span>`s inline in the HTML before it's handed to the frontend. Frontend just injects the HTML into a container — no markdown/highlighting logic in JS.
- **Fuzzy search** — `nucleo` (the fast matcher used by Helix) indexes file paths (and can extend to in-file headings later) for the Telescope-style palette. A Tauri command takes a query string and returns ranked matches.
- **Claude Code integration** — a Tauri command `send_to_claude(snippet, file_path)` shells out to `tmux send-keys -t <configured-pane> "<formatted prompt>" Enter`. This reuses the user's already-running, stateful Claude Code session in its own tmux pane rather than spawning a stateless headless call — it matches how the rest of the workflow already operates. The target pane is configurable.
- **Config** — a TOML file (e.g. `~/.config/dreamd/config.toml`, with a repo-local `.dreamd.toml` override) holds: theme CSS path, tmux target pane, ignore patterns beyond `.gitignore`, and keybinds.

### Frontend (`src-tauri`'s webview target, plain HTML/CSS/JS — no framework needed for v1's scope)

- **Layout**: left sidebar = file tree + search entry point; main panel = rendered markdown; a command-palette-style overlay (triggered by a keybind, e.g. `Ctrl+P`) for fuzzy search, modeled on Telescope's UX.
- **Theming**: a single user-editable `theme.css` (path from config) styles the whole app. The backend's file watcher hot-reloads it in the webview on save — no rebuild needed to reskin.
- **Send-to-Claude flow**: user selects text in the rendered preview, triggers a keybind or context-menu item, frontend calls the `send_to_claude` Tauri command with the selection and current file path.

### Tauri commands / events (the IPC contract)

- `list_markdown_files(repo_root) -> FileNode[]`
- `render_markdown(path) -> String` (HTML, already syntax-highlighted)
- `fuzzy_search(query) -> FileNode[]`
- `send_to_claude(snippet, file_path) -> Result<()>`
- Events pushed from Rust: `file-added`, `file-changed`, `file-removed`, `theme-reloaded`

### Key crates

`tauri`, `pulldown-cmark`, `syntect`, `ignore`, `notify`, `nucleo`, `serde`/`serde_json`, `toml`, `dirs`

### Project layout to create

```
dreamd/
  Cargo.toml
  src-tauri/
    Cargo.toml
    tauri.conf.json
    src/
      main.rs
      fs_walk.rs        # ignore-crate repo scan -> FileNode tree
      watcher.rs         # notify-crate file watching -> Tauri events
      markdown.rs        # pulldown-cmark + syntect render pipeline
      search.rs           # nucleo fuzzy index/query
      claude_bridge.rs    # tmux send-keys integration
      config.rs           # TOML config load (global + repo-local override)
  ui/
    index.html
    app.js
    theme.css             # user-editable, hot-reloaded
  README.md               # setup, keybinds, config format, Linux webkit2gtk dependency note
```

## Verification

1. `cargo tauri dev` — window opens without spawning a browser tab.
2. Point it at a test repo containing nested markdown files, a `.gitignore` excluding a subfolder, and a fenced code block — confirm: tree shows only non-ignored `.md` files, code block is syntax-highlighted, tables/task-lists render correctly.
3. Edit a markdown file from another tmux pane in Neovim, confirm the tree/preview updates live via the watcher.
4. Trigger the fuzzy-search overlay, confirm ranked results and file open on select.
5. Edit `theme.css`, confirm the webview restyles without an app restart.
6. With a `claude` CLI session running in a named tmux pane, select a snippet in the preview and trigger send-to-Claude; confirm the snippet+prompt lands in that pane.
7. `cargo tauri build` — confirm resulting binary size is small (no bundled Chromium) and runs standalone on both macOS and Linux (note the `webkit2gtk` runtime dependency for Linux in the README).
