# dreamd

Lightweight, cross-platform (macOS/Linux) **GUI** markdown reader built for a
tmux + Neovim + Claude Code workflow. The point is the *reading experience*
(typography freed from the terminal) plus a **highlight → annotation → agent**
loop: highlight passages as evidence, attach a question to each, build up a
stack, then push the selected pairs to your agent in one action.

Rust backend (Tauri, its own window — not a browser tab), file-tree + fuzzy
search explorer, CSS-themeable, read-only viewer.

**dreamd never writes to your markdown, and never writes anything inside the
repo.** Editing stays in Neovim. Everything it does persist lives under
`~/.config/dreamd/`, as plain files and no database: your preferences
(`config.toml` and saved themes), and your marks — highlights, the questions
attached to them, and the stack — in `marks/<repo>-<hash>.json`, mode `0600`,
written debounced and flushed on quit. Marks persisting is what lets an agent
loop span more than one session; see [Marks on disk](#marks-on-disk) for the
details, including which dreamd wins when two are open on the same repo.

## Install

### macOS

```sh
brew install --cask bongofongo/tap/dreamd
```

or, without Homebrew:

```sh
curl -fsSL https://raw.githubusercontent.com/bongofongo/dreamd/main/packaging/install.sh | sh
```

Either way you get `dreamd.app` in `/Applications` **and** `dreamd` on your
`PATH` — they are the same executable, so the window and the command line can
never be different versions of each other. Builds are signed and notarized;
`.zip`s per architecture are attached to each
[release](https://github.com/bongofongo/dreamd/releases).

Double-clicking the app opens no window: with no repo to show there is nothing
to show, so it waits in the Dock with its menubar. **File ▸ Open Folder…**
(`⌘O`) picks one. From a terminal, `dreamd` in a repo behaves as it always has.

### Linux

`x86_64` only for now. Three channels, all from the same
[release](https://github.com/bongofongo/dreamd/releases):

```sh
# 1. AppImage — no install, runs anywhere with a WebKitGTK runtime
chmod +x dreamd-*.AppImage && ./dreamd-*.AppImage

# 2. Debian/Ubuntu
sudo dpkg -i dreamd-*.deb

# 3. anything else, no root: the binary into ~/.local/bin, desktop entry
#    and icons into ~/.local/share
curl -fsSL https://raw.githubusercontent.com/bongofongo/dreamd/main/packaging/install.sh | sh
```

Arch: render `packaging/PKGBUILD.tmpl` (the `aur` job in
`.github/workflows/release.yml` does it with the release's version and checksum)
and `makepkg -si`.

There is no signing or notarization on Linux — nothing to strip, nothing to
prompt. The `.sha256` files next to each artifact are the integrity check, and
`install.sh` verifies them before it unpacks anything.

The menubar differs from macOS's on purpose: GTK draws File, Edit and Help only,
and **Open Folder…** is `Ctrl+Shift+O` rather than `Ctrl+O`, which is already
bound to the stack panel inside the window.

## Requirements (building from source)

- Rust (stable) + the Tauri CLI: `cargo install tauri-cli --version "^2"`
- **Linux only:** the WebKitGTK runtime and the GTK3 toolkit. macOS uses the
  system WKWebView and needs no extra deps.
  - Debian/Ubuntu: `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libssl-dev
    patchelf`
  - Arch: `webkit2gtk-4.1 gtk3 librsvg openssl patchelf`
  - Fedora: `webkit2gtk4.1-devel gtk3-devel librsvg2-devel openssl-devel patchelf`
- Optional, and only to build the **release artifacts** rather than the binary:
  `dpkg` — `packaging/build.sh` shells out to `dpkg-deb`, which Debian and Ubuntu
  have already and Arch and Fedora do not.
- Optional: `tmux` — not required, but it upgrades send-to-Claude to zero-paste.

### If live reload stops working on Linux

dreamd watches the repo recursively. That is one FSEvents stream on macOS but
one **inotify watch per directory** on Linux, drawn from a machine-wide budget
that an editor's LSP or a bundler is also spending. A repo deep enough to exhaust
it makes dreamd print `failed to watch …` at startup and then never notice a
save. Raise the budget:

```sh
sudo sysctl fs.inotify.max_user_watches=524288   # persist in /etc/sysctl.d/
```

### If the AppImage build fails on a recent distro

`packaging/build.sh x86_64-unknown-linux-gnu` fails at the AppImage step with a
bare `failed to run linuxdeploy` (Tauri swallows the tool's stderr, so that is
all you get). Run it with `NO_STRIP` set:

```sh
NO_SIGN=1 NO_STRIP=1 packaging/build.sh x86_64-unknown-linux-gnu
```

linuxdeploy ships its own binutils `strip`, old enough that it cannot parse the
`SHT_RELR` sections (`.relr.dyn`, type `0x13`) that distributions now emit when
they build with packed relative relocations. It then fails on **every** system
library it copies into the AppDir. `NO_STRIP=1` skips that pass, which costs
some AppImage size and nothing else.

CI never hits this: it builds on `ubuntu-22.04`, whose libraries predate the
change. That is the same reason the release is built there — so the variable is
a local-build convenience, not something `release.yml` needs.

Rerunning after a failure needs a clean tree — `rm -rf
target/x86_64-unknown-linux-gnu/release/bundle`. linuxdeploy's GTK plugin
`ln -s`es into the AppDir and aborts on a symlink that already exists, so a
second run over a half-populated AppDir fails for a different reason than the
first.

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
cargo tauri build                        # bare binary + dreamd.app
NO_SIGN=1 packaging/build.sh aarch64-apple-darwin   # the full release artifact
```

`packaging/build.sh` is the entire release pipeline and runs the same locally as
in CI — see **Releasing** below.

## Performance

Measurement lives in `perf/`. `.github/workflows/perf.yml` runs the quick tier
on both platforms, but a shared runner is not a quiet machine: its numbers gate
nothing and move no baseline. The measurements that count are local.

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
- **Collapse the tree:** press `Ctrl+B`, or click the `◀` arrow in the tree header; a floating `▶` at the left edge brings it back. The preview stays full-width.
- **Top bar** (on the traffic-light row): contents, print, settings,
  highlighter, stack and send — all icons.
- **Print / save as PDF:** the printer icon opens your OS print dialog over the
  open document; pick **Save as PDF** as the destination to export it. What
  prints is the document alone — no sidebar, no panels, no copy buttons, black
  on white whatever theme you read in, and the whole file however far down it
  you had scrolled. Highlights print as plain text: they are session state, and
  the export is meant to be the document. dreamd chooses no filename and writes
  nothing itself; the save is entirely your dialog's.
- **Find in the document:** `/` opens a search bar at the foot of the reading
  pane. Type, then press `Enter` — nothing highlights until you do, so the page
  stays still while you type. `Enter` jumps to the first match from where you
  are and leaves the bar open; `n` and `N` step forward and back from there.
  **The bar being open is exactly what makes the highlights visible**, so `Esc`
  or the `✕` button closes it and clears every trace of the search in one go.
  Lowercase searches ignore case, a capital anywhere makes it exact (vim's
  `smartcase`). There is no regex switch: your pattern is searched literally,
  and only re-read as a regular expression if the literal finds nothing — so
  `app.js` finds `app.js` rather than quietly also matching `appXjs`, while
  `\bread\b` or `(one|two)` do what you obviously meant. When it does fall back,
  the bar says `regex` so you can see it happened. It searches the document as
  rendered, so a match spanning `**bold**` is found the way you see it, and
  markdown syntax never matches. One file at a time — searching the whole repo's
  contents is not built; the palette finds files by *name*.
- **View mode:** `Ctrl+M` hides the top bar, the tree and both side panels at
  once, leaving just the document. `Ctrl+M` again or `Esc` brings the chrome
  back exactly as you left it — view mode never changes what you had collapsed
  or open underneath. The palette and settings still open over it.
- **Contents:** the outline icon (or `Ctrl+I`) opens a panel listing the open
  document's headings, indented by level; click one to jump to it. It follows
  the file while it is open, so a `:w` in Neovim updates it with the document.
  In-document `[link](#a-heading)` links work too — headings carry GitHub-style
  slug ids.
- **Links:** a relative `[link](notes/other.md)` opens that file in place, and
  `[link](notes/other.md#a-heading)` opens it *and* jumps to the section. Local
  links that resolve outside the repo root, or to something that isn't markdown,
  are ignored rather than handed to the OS. `http(s)` and `mailto:` links open
  in your browser or mail client.
- **Highlight → annotate:** select text in the preview, press the highlight key,
  type a question/comment, "Add to stack".
- **Send:** press the send key (or the toolbar **Send ▸**) to push the whole
  stack; open the stack panel to cherry-pick which pairs go. Ticks stick — a
  pair you untick stays unticked as you add and remove others, so **Send
  selected** sends the selection you actually built.

### Send behavior

The send key opens dreamd's own agent pane (`Ctrl+T` toggles it independently),
starting a Claude Code session on first use, and submits the stack there. The
conversation is drawn by dreamd — the answer is typeset like the document, not
painted into a terminal.

The older path out to tmux is still here, unbound and absent from the settings
panel, for when you want to compare the two: bind `keymap.send_stack_tmux` by
hand and it does what it always did — if tmux is running and a pane running
`claude` is found (or a pane is pinned in config), dreamd writes the query to a
temp file and types a fixed `read @<file>` prompt into that pane, zero paste;
otherwise the query goes to your clipboard (and stays as a temp file) so you can
paste it anywhere. Either way your highlighted text is never interpolated into a
shell command.

## Keybinds (defaults)

| Action                      | Key                        |
|-----------------------------|----------------------------|
| Open a folder *(macOS menu)*| `⌘O`                       |
| Open a file *(macOS menu)*  | `⌘⇧O`                      |
| Open file palette           | `Ctrl+F`                   |
| Palette previous / next     | `Ctrl+P` / `Ctrl+N`        |
| Highlight selection         | `h` (or `Ctrl+Shift+H`)    |
| Add annotation (in modal)   | `Ctrl+Y`                   |
| Toggle highlight mode       | toolbar highlighter icon   |
| Toggle contents panel       | `Ctrl+I`                   |
| Toggle file tree            | `Ctrl+B`                   |
| Toggle stack panel          | `Ctrl+O`                   |
| Toggle the agent pane       | `Ctrl+T`                   |
| View mode (hide all chrome) | `Ctrl+M` (`Esc` exits)     |
| Send stack                  | `Ctrl+Enter`               |
| Copy stack to clipboard     | `Ctrl+C`                   |
| Set the mark                | `m`                        |
| Jump to the mark            | `'`                        |
| Jump back / forward         | `Ctrl+[` / `Ctrl+]`        |
| Open settings               | `Ctrl+,`                   |

Select text (a normal OS selection) and press `h` to turn it into a dreamd
highlight and add an annotation. The highlighter-icon **mode** is optional: when
on, simply finishing a selection auto-starts the same flow — no key needed.
`Ctrl+C` copies the stack only when nothing is selected — with a selection it
falls back to the normal OS copy.

The first two are native menu items, not dreamd keybinds, and they are not
rebindable. They do not collide with `Ctrl+O`: modifier matching is exact, so a
`⌘` chord never reaches a `Ctrl` binding.

**The mark** is vim's, cut down to one: `m` remembers the file you are reading
and where you are in it, `'` returns there — from anywhere, including a
different file. There is no letter to type and no second mark; `m` again moves
the mark to where you are now. Nothing is written to disk: the mark lives for as
long as the app does and is gone when you quit, like highlights and the stack.

**Jump back** (`Ctrl+[`) returns to where you were before something moved you:
a link, a section link, a click in the tree, the palette, the contents panel,
`]`/`[`, or a jump to the mark. `Ctrl+]` undoes a jump back, and any new jump
clears the forward trail. Scrolling with the wheel or the keyboard is *not* a
jump — the trail records places you were teleported away from, not every screen
you passed through. It holds the last 64 of them, and like everything else here
it dies with the app.

Rebind any of these in the settings panel, or in config. The bare `h` is an
alias kept from before keybinds were configurable; turn it off with
`quick_highlight = false` if you'd rather have the letter back.

## Settings

`Ctrl+,` (or the gear in the titlebar) opens the settings panel. Three tabs:

- **Keys** — click a shortcut to record a new one. A picker at the top spells
  the primary modifier three ways — Ctrl, Cmd, or none at all (`Ctrl+F` becomes
  a bare `f`) — which is a rendering of the same map, not a second one.
  Duplicates are flagged, and a shortcut a repo-local `.dreamd.toml` overrides
  is marked as such, so the panel never claims a change took effect when it
  didn't.
- **Themes** — a Light / Dark / System toggle, then every bundled and saved
  theme with a swatch. The toggle is independent of the theme, since every
  theme ships both; the swatch shows the appearance you are currently in. Click
  a card to preview live, Apply to keep. Code-block colours only change on
  Apply, since they are produced server-side.
- **Custom theme** — a colour picker and a text field per palette variable, plus
  the raw CSS. Variables are shown one appearance at a time: the shared block
  plus whichever of light/dark you are editing. Edits preview as you make them;
  Save writes the palette to `~/.config/dreamd/themes/` and switches to it.

Everything the panel writes goes through the same code path as
`dreamd config set`, so a change made here and one made from the shell produce
the same file.

## Config

Global `~/.config/dreamd/config.toml`, overridden by a repo-local `.dreamd.toml`.
All fields optional:

```toml
theme = "tokyo-night"                    # see `dreamd theme list`
mode = "system"                          # or "light" / "dark"
# theme_css = "/path/to/your.css"        # or a complete stylesheet of your own
extra_ignores = ["vendor", "*.tmp.md"]
tmux_target = "work:0.1"                 # pin a pane; skips auto-detect
tmux_autodetect = true

[keymap]
mode = "linux"                           # how Ctrl is spelled: linux / mac / vim
palette = "Ctrl+F"
palette_prev = "Ctrl+P"
palette_next = "Ctrl+N"
highlight = "Ctrl+Shift+H"
send_stack = "Ctrl+Enter"
toggle_stack = "Ctrl+O"
toggle_outline = "Ctrl+I"
toggle_pane = "Ctrl+T"                   # show / hide the agent pane
toggle_tree = "Ctrl+B"
toggle_view = "Ctrl+M"                   # hide all chrome; Esc also exits
toggle_mode = "Ctrl+Shift+D"             # light <-> dark; System is in the panel
jump_top = "Home"                        # scroll the document to the start
jump_bottom = "End"                      # ...and to the end
scroll_down = "j"                        # a line at a time, vim's keys
scroll_up = "k"
scroll_half_down = "d"                   # ...and half a screen at a time
scroll_half_up = "u"
pane_left = "Ctrl+H"                     # move focus between tree, document, panel
pane_right = "Ctrl+J"
next_file = "]"                          # next file in the sidebar's order
prev_file = "["                          # ...previous; both wrap at the ends
set_mark = "m"                           # bookmark this spot (one mark, global)
jump_mark = "'"                          # ...and go back to it
jump_back = "Ctrl+["                     # undo the last jump, wherever it came from
jump_forward = "Ctrl+]"                  # ...and redo it
find = "/"                               # find in the open document
find_next = "n"                          # next match; works with the bar closed
find_prev = "Shift+N"                    # ...previous; both wrap
copy_stack = "Ctrl+C"
settings = "Ctrl+,"
save_annotation = "Ctrl+Y"
quick_highlight = true                   # also accept a bare `h` for highlight
# send_stack_tmux = "Ctrl+Alt+Enter"     # send down the tmux path instead; unbound

[agent]
position = "right"                       # where the Claude Code pane docks; or "bottom"
permission_mode = "accept-edits"         # or "default" / "plan" / "bypass-permissions"
popout = "never"                         # or "send" / "always": float over the reader

[ui]
tree_width = 260                         # sidebar width in px, 140–600
stack_width = 280                        # stack panel, 200–720
pane_width = 380                         # agent pane docked right, 240–1200
pane_height = 240                        # ...and docked bottom, 120–1200
menubar = false                          # the native File / Edit / Help bar
titlebar = false                         # the WM's bar; defaults on (and inert) on macOS
titlebar_fade = true                     # macOS only: dissolve dreamd's own top bar
```

The four sizes are where your drag handles left them, and a number outside the
range is clamped rather than rejected — a stale width costs you the nearest
usable panel, not the rest of the file. The pane keeps a width *and* a height
because its handle changes axis with `position`.

The chrome keys live under Settings → Window, where they apply immediately.
`menubar` is off everywhere and read on Linux only — on macOS the menubar
belongs to the application rather than to the window. `titlebar` is off on Linux
and on by default on macOS, which draws it as an overlay that costs no vertical
space; it is inert there in any case, since there is no such bar to reclaim —
only the traffic lights, which stay. Turning the menubar off takes its
`Ctrl+Shift+O` / `Ctrl+Alt+O` with it: the bar is detached rather than hidden,
which is the only state GTK will not undo the next time the window is shown.
Click the repo name above the file tree to move to another folder without it. A
window with no titlebar is still movable — drag the top few pixels of it.

`titlebar_fade` is not the window's frame at all — it is how dreamd paints a row
of its own page, so it is CSS and the native window knows nothing about it. On
by default on macOS, where dreamd's bar is the only bar there is; offered
nowhere else, where the WM already draws one above it.

Every `[agent]` key is read when the pane opens, so changing one takes effect
on the next cold start rather than mid-session. `send_stack_tmux` is unbound and
absent from the settings panel: the embedded pane is the send path, and this is
the escape hatch back to `tmux send-keys` when you want to compare them.

The repo-local file overrides the global one key by key, so a `.dreamd.toml` that
sets one thing leaves the rest of your setup alone. It may name a `theme` but
cannot set `theme_css` — a cloned repo does not get to point dreamd at an
arbitrary file on your disk — and it cannot set `agent.permission_mode`, for the
same reason: a repo you have not read yet does not get to decide what your agent
may do without asking. Nor `ui.menubar` or `ui.titlebar`: your window frame is
part of your desktop, and a repo that could take the close button off it is not
setting a preference. It may still resize the tree.

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
holds the variables. Ten themes ship in the binary, each carrying **both a dark
and a light appearance**:

| | |
|---|---|
| `dreamd` | the default — serif, paper by day, indigo by night |
| `manuscript` | warm sepia desk / vellum by candle |
| `letterpress` | ink on cotton, justified, high contrast |
| `athenaeum` | reading room / library at night, brass on green-black |
| `gruvbox` `catppuccin` `tokyo-night` `nord` `solarized` `high-contrast` | the programmer-coded ones |

```sh
dreamd theme list                        # bundled + yours, active marked
dreamd theme set nord
dreamd config set mode light             # light, dark, or system (the default)
dreamd --theme manuscript --mode dark    # this run only
dreamd theme new mine --from nord        # copy into ~/.config/dreamd/themes/
dreamd theme show mine                   # print the full stylesheet
```

`mode` is independent of which theme you picked — every theme has both halves.
The default follows the OS and keeps following it while the app runs.

A palette is a family: a bare `:root` of shared typography, plus a
`:root[data-mode="light"]` and a `:root[data-mode="dark"]` block of colours.
Switching appearance is one attribute on `<html>`, so it is instant.

```css
:root {
  --font-body: ui-serif, "New York", "Iowan Old Style", Charter, Georgia, serif;
  --font-size: 17px;
  --content-width: 700px;
}
:root[data-mode="light"] { --bg: #f7f5fa; --syntax-theme: "InspiredGitHub"; }
:root[data-mode="dark"]  { --bg: #14121c; --syntax-theme: "base16-ocean.dark"; }
```

`--syntax-theme` names the syntect theme for fenced code, per appearance — that
is what keeps code blocks from staying dark under a light theme. A few optional
variables let a theme change shape rather than only colour: `--font-heading`,
`--heading-weight`, `--heading-rule`, `--letter-spacing`, `--para-spacing`,
`--text-align`, `--hyphens`, `--code-bg`, `--hl-text`, `--stale-text`.

A palette written before families existed — one bare `:root`, no mode blocks —
still works, and reads the same in both appearances. The older per-appearance
theme names (`gruvbox-dark`, `catppuccin-latte`, …) still resolve too;
`dreamd theme set <old-name>` rewrites your config into the new spelling.

Palettes in `~/.config/dreamd/themes/` hot-reload on save; bundled ones are
embedded in the binary and need a rebuild.

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

## Marks on disk

Highlights, annotations and the stack are written to
`~/.config/dreamd/marks/<repo-basename>-<16hex>.json` (mode `0600`), debounced,
and flushed when you quit. They are keyed by repo, so opening a different repo
shows that repo's marks and nothing else. Nothing is written inside the repo
itself.

Marks read back off disk are re-anchored **lazily, once per file, the first time
you open it** — not all at once at startup, which would land straight on the
cold-start time. A mark that no longer locates reads as stale, exactly as one
made this session would.

**Two dreamds on one repo.** The first to open a repo claims it, by binding that
repo's socket under `~/.config/dreamd/run/`. Only the holder writes marks; a
second window keeps its marks in memory for as long as it is open and says so on
stderr rather than racing the first one's file. Close the first and the second
does *not* silently take over — the claim is decided when a repo is adopted.

```sh
dreamd marks path                  # where this repo's marks file is
dreamd marks prune                 # dry run: reports what each flag would remove
dreamd marks prune --stale         # stale AND unannotated — what a checkout stranded
dreamd marks prune --older-than 30d  # answered longer ago than that (30d/12h/90m)
```

`prune` is a destructive verb with a read-only default, so the first thing you
type is a dry run. The two flags are not the same set: `--stale` removes marks
nobody ever asked a question about, `--older-than` removes ones that were asked
*and answered*.

A corrupt or unreadable marks file costs you the marks, never the launch.

## Status / known v1 limits

- Fuzzy search covers file **paths** only; in-file/content (`live_grep`) search
  is a v2 item.
- Highlight anchoring matches on the selected text (whitespace-normalized);
  heavily formatted inline selections may not re-locate and will read as stale.
  Inside fenced code blocks, a highlight spanning more than one syntax token
  anchors correctly but does not *paint*: syntax highlighting splits the line
  into per-token spans, and the painter skips matches straddling a span
  boundary. The mark is real and reaches your agent; you just can't see it.
- Marks persist per repo, but only for the dreamd holding that repo's claim; a
  second window on the same repo keeps its marks in memory only.

## Releasing

`packaging/build.sh` is the whole pipeline; `.github/workflows/release.yml` is a
thin wrapper around it, so a release can be reproduced locally without pushing a
tag.

```sh
packaging/set-version.sh 0.2.0     # src-tauri/Cargo.toml + website/src/consts.ts + Cargo.lock
cargo build
git commit -am "release: 0.2.0" && git tag v0.2.0 && git push && git push --tags
```

The tag builds both architectures, signs and notarizes them, and opens a
**draft** release. Verify the app on a clean machine, then publish it by hand —
publishing is what bumps the Homebrew cask, so nothing reaches users until
someone has actually double-clicked the thing.

Secrets the workflow needs: `APPLE_CERTIFICATE` (base64 of the Developer ID
**Application** `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_ID`, `APPLE_PASSWORD` (an *app-specific* password), `APPLE_TEAM_ID`, and
`TAP_GITHUB_TOKEN` (fine-grained, `contents: write` on the tap repo only).

### macOS permissions, and why they are the way they are

dreamd is not sandboxed — App Sandbox is a Mac App Store requirement, not a
notarization one — and ships with **no entitlements file**. Two deliberate
choices keep it that way:

- `delete_file` uses `trash`'s `NsFileManager` backend rather than its default
  of asking Finder over Apple Events, so no automation permission is needed. The
  cost is that Trash's "Put Back" doesn't appear; recovery is dragging the file
  out of the Trash.
- The dmg is not built. Tauri's dmg bundler runs an AppleScript to pose the
  Finder window and dies with `AppleEvent timed out (-1712)` without
  Automation → Finder permission. The `.zip` is what the cask and the installer
  consume anyway.

`Info.plist` carries folder-usage strings for Documents/Desktop/Downloads
because a repo in one of those is normal and the walk will trip TCC. Note the
asymmetry: launched from Finder, TCC attributes the request to `dreamd.app` and
shows those strings; launched through the `PATH` symlink from a terminal, it
generally attributes to the terminal and inherits whatever that already has.

## Licence

Apache License 2.0 — see [LICENSE](LICENSE).
