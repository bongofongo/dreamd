# Session log

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
