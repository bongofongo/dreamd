# Theming dreamd

This is the sheet an agent reads before changing how dreamd looks. `dreamd
theme guide` prints it, followed by the generated lists (variables, syntect
themes, bundled themes, the selector surface). `dreamd theme guide --json`
prints the same facts as one JSON document.

## The loop

```sh
dreamd theme guide                    # this document
dreamd theme new mine --from dreamd   # copy a bundled palette to ~/.config/dreamd/themes/mine.css
$EDITOR ~/.config/dreamd/themes/mine.css
dreamd theme check mine               # errors → exit 1; fix them before setting
dreamd theme set mine                 # writes theme = "mine" to config; a running dreamd restyles at once
dreamd config set mode light          # light | dark | system (the default)
```

A running dreamd hot-reloads a user palette on every save, so an agent editing
the file while the reader watches is the intended workflow. Nothing here needs
the window closed, and nothing here writes anywhere but `~/.config/dreamd/`.

## What a theme is

Two layers. **`ui/theme.css`** (embedded in the binary) holds the reading
*rules* — how a heading, a code block, a blockquote is laid out. A **palette**
holds the *variables* those rules and the window chrome read. The palette is
appended after the base rules, so every variable it declares wins.

A palette is a **family**: one file carrying both appearances.

```css
:root {                      /* shared: type metrics */
  --font-body: ui-serif, "New York", Charter, Georgia, serif;
  --font-mono: ui-monospace, "SF Mono", Menlo, monospace;
  --font-size: 17px;
  --line-height: 1.75;
  --content-width: 700px;
  --ui-font-size: 14px;
}
:root[data-mode="light"] {   /* colours, one block per appearance */
  --bg: #f7f5fa;  --sidebar-bg: #f0edf6;  --btn-bg: #e8e4f0;  --hover: #e0dbec;  --border: #ddd7e6;
  --text: #2f2b3a;  --muted: #6a6379;  --link: #6a4fb3;
  --accent: #7c5cd6;  --accent-dim: #e6dffa;
  --hl: #f2d16b;  --hl-prior: 24%;  --stale: #c2455c;  --stale-bg: #fbe3e7;
  --syntax-theme: "InspiredGitHub";
}
:root[data-mode="dark"] {
  --bg: #14121c;  --sidebar-bg: #100e17;  --btn-bg: #1f1c2b;  --hover: #292435;  --border: #2e2940;
  --text: #d8d3e4;  --muted: #8f88a3;  --link: #a99bf0;
  --accent: #a48cf5;  --accent-dim: #2b2447;
  --hl: #f2d16b;  --hl-prior: 6%;  --stale: #ef6a7d;  --stale-bg: #3a1a22;
  --syntax-theme: "base16-ocean.dark";
}
```

That example is complete: it declares every required variable and passes
`dreamd theme check`. The frontend switches appearance by setting `data-mode`
on `<html>`, so `mode` and `theme` are independent — every theme has both
halves, and `mode = "system"` follows the OS while the app runs.

Declare variables on `:root`, not on `body`. The page background is painted on
`<html>`, and the native window behind it is painted by Rust from `--bg`
before the webview exists.

A palette with no `[data-mode]` blocks — one bare `:root` — still works and
reads the same in both appearances. `check` warns about it and nothing else.

## Three values Rust reads, not CSS

- **`--bg`** must be a hex colour (`#rgb` or `#rrggbb`). It is parsed to paint
  the native window and the webview's canvas before the stylesheet lands, so
  a light theme flashes light. `rgb()`, `hsl()` and named colours are an error.
- **`--syntax-theme`** names the [syntect](https://github.com/trishume/syntect)
  theme that colours tokens in fenced code. It is quoted, per mode, and must be
  one of the names the guide lists — an unknown name silently falls back to
  `base16-ocean.dark`, which is why `check` makes it an error. The *slab*
  behind the code is the palette's (`--btn-bg`, or `--code-bg`); only the
  token colours come from syntect, so `--syntax-theme` has to get polarity
  right and nothing more.
- **`--hl-prior`** is a percentage: how much of `--hl` survives on a highlight
  made in an earlier session. Per mode, and the two must differ — a strength
  that whispers on paper is a lit bar on black. Undeclared, it falls back to
  16%.

## The variables

Required means the base stylesheet or the chrome has a fallback, but the
fallback belongs to the default theme — a palette that omits one is painting
part of the window in someone else's colour. `check` reports each missing one
per appearance.

<!-- variables -->
| variable | kind | block | required | paints |
|---|---|---|---|---|
| `--font-body` | font stack | shared | yes | the document's prose |
| `--font-mono` | font stack | shared | yes | inline code, fenced blocks, the terminal pane, and every monospaced field in the chrome |
| `--font-size` | length | shared | yes | the document's base font size; `ui.zoom` multiplies it |
| `--line-height` | number | shared | yes | the document's line height |
| `--content-width` | length | shared | yes | the document's measure — its max width; `ui.zoom` multiplies it so the width in characters stays put |
| `--ui-font-size` | length | shared | yes | the chrome's font size: sidebar, buttons, panels, modals |
| `--font-ui` | font stack | shared | no | the chrome's font family; defaults to the system UI face |
| `--font-heading` | font stack | shared | no | headings, when they should differ from `--font-body` |
| `--heading-weight` | number | shared | no | heading font weight (default 650) |
| `--heading-rule` | border | shared | no | the rule under h1 and h2; `none` for a reading theme (default `1px solid var(--border)`) |
| `--para-spacing` | length | shared | no | vertical margin between paragraphs (default 0.8em) |
| `--text-align` | keyword | shared | no | paragraph alignment; `justify` wants `--hyphens: auto` beside it |
| `--hyphens` | keyword | shared | no | `manual` (default) or `auto` |
| `--letter-spacing` | length | shared | no | tracking on the document's prose |
| `--bg` | colour | per mode | yes | the page and the native window behind it — hex only, Rust paints the window from it |
| `--sidebar-bg` | colour | per mode | yes | the sidebar, the stack panel, the agent pane and card, the find bar, menus and the outline |
| `--btn-bg` | colour | per mode | yes | buttons, inputs, table headers, inline code and the code-block slab (unless `--code-bg`) |
| `--hover` | colour | per mode | yes | hover state on buttons, tree rows, menu items, and the agent's tool cards |
| `--border` | colour | per mode | yes | every 1px rule: panel edges, buttons, inputs, table cells, heading rules |
| `--code-bg` | colour | per mode | no | the fenced code block's slab instead of `--btn-bg`; `transparent` restores syntect's own |
| `--text` | colour | per mode | yes | body text, in the document and the chrome |
| `--muted` | colour | per mode | yes | secondary text: blockquotes, hints, placeholders, timestamps |
| `--link` | colour | per mode | yes | links in the document |
| `--accent` | colour | per mode | yes | the primary button, the blockquote bar, the caret, focus rings, selected tabs' edges |
| `--accent-dim` | colour | per mode | yes | a wash of the accent: the open file in the tree, the selected palette row, tab and theme card |
| `--hl` | colour | per mode | yes | the highlight fill on a marked passage and the highlight-mode button |
| `--hl-text` | colour | per mode | no | text on a highlight fill (default near-black) |
| `--hl-prior` | percent | per mode | yes | how much of `--hl` survives on a mark from an earlier session; differs per mode because the same strength is a whisper on paper and a lit bar on black |
| `--stale` | colour | per mode | yes | a mark whose passage was edited away: the rail chip's edge, the danger button, error text |
| `--stale-bg` | colour | per mode | yes | the stale chip's fill and the danger button's hover |
| `--stale-text` | colour | per mode | no | text on a stale fill (default near-black) |
| `--syntax-theme` | syntect theme | per mode | yes | the syntect theme for tokens in fenced code, quoted; one per mode or a light theme gets dark code |
| `--radius` | length | shared | no | corner radius on buttons, inputs, chips, inline code and images (each site defaults to 4–6px) |
| `--radius-lg` | length | shared | no | corner radius on cards, modals, the agent pop-out, menus and code blocks (each site defaults to 8–12px) |
| `--shadow` | box-shadow | shared | no | the shadow under every floating surface — modals, menus, the pop-out, the toast; `none` flattens them |
<!-- /variables -->

"Block" is where the variable belongs in a family; `check` resolves every
variable per appearance through the same slice the app uses, so a colour in
the shared block passes — it is just a colour that cannot differ by mode.

The shape variables are the way to make dreamd *rounder*, *flatter* or *set in
a different UI face* without writing a stylesheet: each site keeps its own
default when the variable is unset, so a palette that never mentions
`--radius` is pixel-identical to one written before it existed.

## What `check` holds a file to

Errors (exit 1): a required variable missing from either appearance; a `--bg`
that is not hex; a `--syntax-theme` that is not a syntect theme; an
`--hl-prior` that is not `N%`; in a family, `--bg`, `--syntax-theme` or
`--hl-prior` identical in both modes.

Warnings (exit 0): a declared `--variable` dreamd does not read, with the
nearest real name — a typo paints nothing and says nothing, so this is the
one an agent should take seriously; `--text` or `--link` under 4.5:1 against
`--bg` (Nord's links sit there on purpose, which is why it is a warning); a
palette with no mode blocks.

`check` takes a theme name (user file first, then bundled) or a path to a
`.css` file that is not installed yet, and defaults to the active theme.

## Going further: a whole stylesheet

`theme_css = "/path/to/file.css"` in `config.toml` (`dreamd config set
theme_css /path`) replaces `ui/theme.css` with your file and appends **no
palette**. What you write is the whole reading stylesheet, so start from
`dreamd theme show <name>` — the base rules plus a palette, which is exactly
what the app would otherwise inject — and edit that.

The window chrome is not replaced. Its stylesheet is in `ui/index.html`, and
your file is injected **after** it, in `<style id="user-theme">`, so a rule of
equal specificity in yours wins. Everything the chrome styles is reachable by
id or class, and the guide ends with the full list. The regions:

| region | selectors |
|---|---|
| the window grid | `#workspace` (2×2: sidebar spans both rows; `#titlebar` and `#main-wrap` stack in column 2) |
| sidebar | `#sidebar`, `#sidebar-header`, `#repo-name` (the root field), `#tree`, `.tree-item` (`.file`, `.active`), `.tree-dir` > `.tree-label`, `#tree-resize` |
| title bar | `#titlebar`, `#tb-actions` (its buttons: `#btn-outline` `#btn-pane` `#btn-settings` `#btn-hl-mode` `#btn-copy` `#btn-stack` `#btn-send`, styled as a group), `#stack-badge` |
| the document | `#content-scroll` (the scroller), `#content` (the rendered markdown; `max-width: var(--content-width)`), `.code-block` and `button.code-copy` around every fence, `#stale-rail` and `.stale-chip` |
| marks | `mark.hl` (a highlighted passage; several `<mark>`s may share one `data-id`), `mark.hl[data-prior]` (from an earlier session), `mark.hl.stale`, `[data-run="start|mid|end"]` on the slices of a passage that crosses elements |
| find | `#find-bar`, `#find-input`, `#find-count`; match painting is `::highlight(find-match)` and `::highlight(find-current)` |
| outline | `#outline-panel`, `#outline-list`, `.oi` |
| the stack | `#stack-panel`, `#stack-list`, `.pair`, `#stack-actions`, `#stack-resize` |
| the agent pane (dock) | `#pty-pane`, `#pty-head`, `#pty-status`, `#pty-mcp`, `button.pty-model`, `#pty-term` (the terminal surface) |
| the conversation | `#agent-body`, `#agent-log`, `.agent-turn`, `.agent-said`, `.agent-tool` (`.failed`), `.agent-note`, `.agent-card` (a permission card; `.settled`), `#agent-composer`, `#agent-input`, `#agent-send` — written unscoped because the same subtree is *moved* between the dock and the pop-out |
| the agent pop-out | `#agent-popout`, `#agent-card`, `#agent-hint` |
| command palette | `#palette-overlay`, `#palette` (and `#palette input`), `#palette-results`, `.pr` (`.sel`) |
| annotation modal | `.modal-overlay` (`#annot-overlay`, `#confirm-overlay`, `#settings-overlay` all carry it), `#annot-box` and its `textarea` and buttons (`#annot-text`, `#annot-save` `#annot-delete` `#annot-resize` `#annot-cancel`) |
| confirm modal | `#confirm-box` (`#confirm-ok`, `#confirm-cancel`) |
| settings | `#settings-box`, `#settings-tabs`, `.st-tab` (`.sel`), `#settings-body`, `.th-card` (`.sel`) and `.th-swatch`, `.st-var` |
| file menu | `#file-menu`, `.fm-item` |
| zoom pill, lightbox, toast | `#zoom-pill`, `#lightbox` and `#lightbox-bar`, `#toast`, `#resize-hint`, `#send-bar`, `#tooltip` |
| modes on `<body>` | `.nav-collapsed` (sidebar hidden), `.view-mode` (all chrome hidden), `.chrome-fade` (the fading title bar), `.hl-mode` (highlight mode), `.pane-open` with `.agent-right`, `.mac` |

Four things a stylesheet has to respect:

- Sizes dreamd sets **inline** from config or measurement win over any
  stylesheet: `--zoom` on `<html>`; `--tree-width`, `--stack-width`,
  `--pane-width`, `--pane-height` on `#workspace`; `--img-w` per image. Change
  those through `config.toml`'s `[ui]` keys, not CSS.
- The fenced code block's background is `!important` in the base rules,
  because syntect writes its own as an inline style on the `<pre>`. Use
  `--code-bg`, or match the `!important`.
- Keep `color-scheme` honest: `:root[data-mode="light"] { color-scheme: light; }`
  and the dark twin are what keep native scrollbars and form controls from
  staying dark under a light palette.
- Nothing about `@media print` needs restating; `#print-css` is last in
  `<head>` and inert on screen.

`theme_css` hot-reloads too. Changing *which* path it points at needs a restart
to re-arm the watcher.

## What cannot be themed

- Token colours in fenced code beyond syntect's bundled set (listed below).
- The native window frame — the title bar toggle and menubar are `[ui]` config
  keys, not CSS. On macOS `ui.titlebar_fade` is the one window setting that is
  CSS (`body.chrome-fade`).
- Layout: which panel docks where is `agent.position` and `agent.popout`;
  panel widths are the `[ui]` sizes above.

## Trust

A repo-local `.dreamd.toml` may set `theme` (a name) but never `theme_css` (a
path read into a `<style>` tag): a cloned repo does not get to point dreamd at
a file. Bundled palettes are compiled in; a user file of the same name in
`~/.config/dreamd/themes/` shadows it. Theme names are file stems — letters,
digits, `.`, `-`, `_`, at most 64 — and never paths.

## For the reader of this file in the repo

`ui/theme.css` is the base; `ui/themes/*.css` are the bundled families, each
carrying both modes, checked by `cargo run --example theme_check`. The
variable table above is generated from `theme::contract::VARS` and pinned by a
test — edit the const, then `dreamd theme guide --readme > ui/themes/README.md`
to regenerate it. The selector list is scanned from `ui/index.html` at build
time (`build.rs`), so it is never in this file and never stale.
