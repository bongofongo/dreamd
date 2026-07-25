# dreamd

Lightweight, cross-platform (macOS/Linux) **GUI** markdown reader built for a
tmux + Neovim + Claude Code workflow. The point is the *reading experience*
(typography freed from the terminal) plus a **highlight → annotation → agent**
loop: highlight passages as evidence, attach a question to each, build up a
stack, then push the selected pairs to your agent in one action.

Rust backend (Tauri, its own window — not a browser tab), file-tree + fuzzy
search explorer, CSS-themeable, read-only viewer. No session state is persisted:
highlights, annotations, and the stack live in memory and are gone when the
process exits. Your preferences do persist — config and themes under
`~/.config/dreamd/` — and that is the only thing dreamd ever writes.

## Requirements

- Rust (stable) + the Tauri CLI: `cargo install tauri-cli --version "^2"`
- **Linux only:** WebKitGTK runtime — e.g. `webkit2gtk-4.1` (Debian/Ubuntu:
  `libwebkit2gtk-4.1-dev`), plus `libgtk-3-dev`. macOS uses the system WKWebView,
  no extra deps.
- Optional: `tmux` — not required, but it upgrades send-to-Claude to zero-paste.

## Run

```sh
# from the repo you want to read
cargo tauri dev            # dev build, opens the window
# or point it somewhere:
cargo tauri dev -- -- /path/to/repo
```

Once installed as a binary: `dreamd [path]`. Like nvim:

- `dreamd` — browse the current directory's repo (root resolved by walking up to
  the nearest `.git`).
- `dreamd notes/todo.md` — open that file on load, but the file tree is still
  rooted at the repo of the current directory.
- `dreamd some/dir` — browse that directory's repo.

Build a release binary (small — no bundled Chromium):

```sh
cargo tauri build
```

## Performance

Measurement lives in `perf/` and runs entirely locally — no CI.

```sh
./perf/run.sh quick     # ~60s    after an edit
./perf/run.sh pass      # ~5min   before a commit
./perf/run.sh deep      # ~15min  profiling; the only tier that sets the baseline
```

Optional tooling, each skipped with an install hint if absent:

```sh
brew install hyperfine samply
cargo install cargo-bloat
cd perf/harness && npm run setup   # Playwright + Chromium, test-only
```

The Playwright harness is a **test dependency only** — it has its own
`package.json`, `node_modules` is gitignored, and nothing node-related is
referenced by `tauri.conf.json` or enters the binary. Its numbers come from
Chromium, not WKWebView, so they detect regressions but are not the app's real
timings; those come from the instrumented binary (`cargo build --features perf`)
and from Instruments.

See `perf/README.md` for what each tier measures and how much to trust it.

## Usage

- **Open a file:** click it in the tree, or open the fuzzy palette.
- **File options:** hover a file row and click `⋯` → **Copy path** or **Delete** (delete moves it to the OS Trash after a confirm).
- **Collapse the tree:** click the `◀` arrow in the tree header; a floating `▶` at the left edge brings it back. The preview stays full-width.
- **Top bar** (on the traffic-light row): highlighter, stack, and send — all icons.
- **Highlight → annotate:** select text in the preview, press the highlight key,
  type a question/comment, "Add to stack".
- **Send:** press the send key (or the toolbar **Send ▸**) to push the whole
  stack; open the stack panel to cherry-pick which pairs go.

### Send behavior (tmux optional)

1. If tmux is running and a pane running `claude` is found (or a pane is pinned
   in config), dreamd writes the query to a temp file and types a fixed
   `read @<file>` prompt into that pane — zero paste. Your highlighted text is
   never interpolated into a shell command.
2. Otherwise the query is copied to your clipboard (and kept as a temp file) so
   you can paste it into Claude.ai / the desktop app / any agent.

## Keybinds (defaults)

| Action                      | Key                        |
|-----------------------------|----------------------------|
| Open file palette           | `Ctrl+F`                   |
| Palette previous / next     | `Ctrl+P` / `Ctrl+N`        |
| Highlight selection         | `h` (or `Ctrl+H`)          |
| Add annotation (in modal)   | `Ctrl+Y`                   |
| Toggle highlight mode       | toolbar highlighter icon   |
| Toggle stack panel          | `Ctrl+O`                   |
| Send stack                  | `Ctrl+Enter`               |
| Copy stack to clipboard     | `Ctrl+C`                   |

Select text (a normal OS selection) and press `h` to turn it into a dreamd
highlight and add an annotation. The highlighter-icon **mode** is optional: when
on, simply finishing a selection auto-starts the same flow — no key needed.
`Ctrl+C` copies the stack only when nothing is selected — with a selection it
falls back to the normal OS copy.

Override in config.

## Config

Global `~/.config/dreamd/config.toml`, overridden by a repo-local `.dreamd.toml`.
All fields optional:

```toml
theme = "tokyo-night"                    # see `dreamd theme list`
# theme_css = "/path/to/your.css"        # or a complete stylesheet of your own
extra_ignores = ["vendor", "*.tmp.md"]
tmux_target = "work:0.1"                 # pin a pane; skips auto-detect
tmux_autodetect = true

[keymap]
palette = "Ctrl+F"
palette_prev = "Ctrl+P"
palette_next = "Ctrl+N"
highlight = "Ctrl+H"
send_stack = "Ctrl+Enter"
toggle_stack = "Ctrl+O"
copy_stack = "Ctrl+C"
settings = "Ctrl+,"
save_annotation = "Ctrl+Y"
quick_highlight = true                   # also accept a bare `h` for highlight
```

The repo-local file overrides the global one key by key, so a `.dreamd.toml` that
sets one thing leaves the rest of your setup alone. It may name a `theme` but
cannot set `theme_css` — a cloned repo does not get to point dreamd at an
arbitrary file on your disk.

From the shell:

```sh
dreamd config path                       # where the global file lives
dreamd config edit                       # open it in $VISUAL/$EDITOR
dreamd config get keymap.palette
dreamd config set keymap.palette Ctrl+Space
```

`config set` and the settings panel rewrite the file. Values you set by hand are
preserved; comments and key ordering are not.

## Theming

A theme is two files: `ui/theme.css` holds the reading *rules*, and a **palette**
is a bare `:root { --bg: …; }` block. Ten palettes ship in the binary —
`dreamd`, `gruvbox-dark`, `gruvbox-light`, `catppuccin-mocha`, `catppuccin-latte`,
`tokyo-night`, `nord`, `solarized-light`, `high-contrast-dark`,
`high-contrast-light`.

```sh
dreamd theme list                        # bundled + yours, active marked
dreamd theme set nord
dreamd --theme gruvbox-light             # this run only
dreamd theme new mine --from nord        # copy into ~/.config/dreamd/themes/
dreamd theme show mine                   # print the full stylesheet
```

A palette carries colour *and* typography (`--font-size`, `--line-height`,
`--content-width`), plus `--syntax-theme`, which names the syntect theme used for
fenced code — that is what keeps code blocks from staying dark under a light
theme. Palettes in `~/.config/dreamd/themes/` hot-reload on save; bundled ones
are embedded in the binary and need a rebuild.

Setting `theme_css` instead points at a complete stylesheet of your own,
replacing the base rules entirely — no palette is appended. It hot-reloads too.

Note: macOS (WKWebView) and Linux (WebKitGTK) use different web engines, so
pixel-identical rendering across platforms isn't guaranteed — theme against both.

## Highlights and live edits

Edit a file in Neovim while dreamd shows it; on save the preview reloads (scroll
position preserved) and highlights re-anchor. If an edit changed the *highlighted
text itself*, that highlight can no longer be located: it's flagged **stale** —
turned red and pushed to the margin with a `?` ("still pertinent?") to keep or
dismiss.

## Status / known v1 limits

- Fuzzy search covers file **paths** only; in-file/content (`live_grep`) search
  is a v2 item.
- Highlight anchoring matches on the selected text (whitespace-normalized);
  heavily formatted inline selections may not re-locate and will read as stale.
- No persistence by design.
