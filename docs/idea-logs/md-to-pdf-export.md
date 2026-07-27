# Idea log — export current document to PDF

Source idea: `ideas/md-to-pdf-export.md`. Overnight batch, branch
`claude/overnight-ideas-2026-07-27`.

**Decision: split — content-only export implemented, the refinements planned.**
The idea's recommended direction (`window.print()` + an `@media print`
stylesheet, no PDF crate) is genuinely small and genuinely low risk: print rules
are inert on screen, so the worst case for the shipped code is that printing
looks wrong, never that reading does. What was held back is everything that
needed the printed page to have been *seen* — syntax colour, an annotations
appendix, a keybind. That is `docs/plans/md-to-pdf-export.md`.

## The one correction to the idea's recommendation

**`window.print()` does nothing at all in WKWebView.** WebKit routes it to the
UI delegate's `_webView:printFrame:`; wry's `WKUIDelegate`
(`wry/src/wkwebview/class/wry_web_view_ui_delegate.rs`) implements only the
file-upload, media-permission and new-window callbacks, so the JS call returns
having silently printed nothing — on the one platform dreamd is built for.
Tauri's own doc comment on `WebviewWindow::print` claims "`window.print()` works
on all platforms"; on macOS + wry it does not.

So the trigger goes through Rust — `WebviewWindow::print()`, which reaches
`printOperationWithPrintInfo:` / `NSPrintOperation` on macOS and
`WebKitPrintOperation`'s dialog on WebKitGTK. Six lines, no dependency, and it
is the difference between a feature and a dead button. Had this shipped as the
idea literally described it, it would have appeared to work in review and done
nothing on the author's machine.

Everything else in the idea's reasoning held: no PDF crate, no CSP change
(printing is a native action, not a network or script one — `script-src 'self'`
is untouched and the print CSS is a static `<style>`, not an inline one), and
nothing added to `Cargo.toml`.

## The open question, answered

**Content-only. Highlights print as plain text.**

1. **Tenet 2.** Highlights and annotations are in-memory session state that dies
   with the process. A PDF is durable. Baking the ephemeral into the durable is a
   category error, and it makes the export irreproducible — the same file would
   give a different PDF depending on what this session happened to mark up.
2. **A stale highlight prints as a lie.** `Stale` is a claim about whether
   dreamd could still re-anchor a quote in the source. On paper it is a red box
   meaning nothing.
3. **`--hl` is a screen colour.** Yellow-on-white via a printer's greyscale is a
   grey smear over the words it was meant to emphasise.

Not "no", though — the appendix design is in the plan (§2), including why
marginalia is the wrong shape for a paged medium and why an appendix should
filter to the current file.

## The trigger

**A titlebar button, no keybind.** `#btn-print`, between contents and settings,
carrying `data-tip` but no `data-tip-key` — `#btn-hl-mode` is the existing
precedent for a titlebar action with no key of its own.

No keybind because `Ctrl+P` is `Keymap::palette_prev`. The clash is functionally
harmless (`palette_prev` is only read inside the palette input's own
`onkeydown`, and the global handler bails while that overlay is open) but
`comboClashes()` compares raw combo strings across all of `KEY_ACTIONS` and
would paint both rows red in the settings panel. Shipping a default the app's own
settings panel flags as broken is not acceptable, and printing is a
once-a-month action that does not obviously earn a key at all. `KEY_ACTIONS`,
`Keymap`, the harness fixtures and the `rows === 17` assertion are therefore all
**untouched**. Plan §3 ranks the options if a key is wanted later; the File →
Print menu item with `CmdOrCtrl+P` is the one I would pick, because it also
solves printing from view mode.

## What was done

### `ui/index.html` — the button and `#print-css`

A **new, last** `<style id="print-css">` in `<head>`, after `#user-theme` and
`#theme-preview`. Placement is load-bearing: the theme and the user palette are
injected at runtime into `#user-theme`, so a print rule in the structural sheet
above would lose every equal-specificity tie to a rule the theme declares. Being
last wins those ties. The trade — a theme cannot override the print sheet — is
deliberate: tenet 5 makes the *screen* themeable, and a palette designed for a
screen is exactly what must not reach paper.

Five things the block does, each of which is a way this would otherwise have
come out wrong:

- **Colour, at the variable level.** `--bg`, `--text`, `--border`, `--link`,
  `--code-bg`, `--content-width`, `--font-size`, `--line-height` and the rest are
  redefined for print on all three family selectors (`:root`,
  `:root[data-mode="light"]`, `:root[data-mode="dark"]` — the dark one outranks a
  bare `:root` regardless of source order, so each must be matched). Every colour
  in the app resolves through these, so one block repaints the chrome sheet, the
  reading sheet *and* any user palette, with nothing to keep in step later. The
  default theme is dark; without this the document prints light grey on white.
- **The scroller, flattened.** `#content-scroll` is `overflow: auto` inside a
  `100vh` flex column. Left alone, the printed output is the screenful that
  happened to be in view — a document scrolled to 60% prints its middle third.
  `html, body, #workspace, #main-wrap, #content-scroll` all go to normal flow
  with `height: auto; overflow: visible`, so it paginates in full from the top
  whatever the scroll position.
- **Chrome, unconditionally.** Titlebar, sidebar, `#btn-expand`,
  `#outline-panel`, `#stack-panel`, `#stale-rail`, `#progress-rail`,
  `#file-menu`, `#tooltip`, `#toast`, every overlay, and `button.code-copy`.
  `!important`, not source order: several are shown by a *more specific*
  selector (`#outline-panel.open`, `.modal-overlay.open`) that nothing can
  outrank otherwise. It also makes this independent of `body.view-mode` — the
  page is identical whether or not the reader had the chrome hidden, which is
  right, because view mode is a reading preference and not an export setting.
- **Clipping.** `#content pre` and `#content table` scroll horizontally on
  screen; on paper that is silent truncation. Code wraps (`white-space:
  pre-wrap`), the table becomes a real `display: table` at full measure.
- **Typography.** 160mm measure centred inside a 16mm `@page` margin — fits A4
  and Letter, ~80 characters at 11pt. Sparse pagination hints: `break-after:
  avoid` on headings, `break-inside: avoid` on images only (on a block taller
  than a page it pushes a blank page instead), `orphans`/`widows: 3`, with the
  legacy `page-break-*` aliases alongside since neither can be tested here.
  External `http(s)` links get their URL appended via `::after`, because a link
  is dead on paper and the href is the only thing that revives it.

Syntax colour is deliberately dropped: `#content pre span { color: inherit
!important }`. syntect writes token colours as inline styles and author
`!important` is the only thing that outranks them, so it is all-or-nothing from
CSS alone — and a dark palette's pastels on white are unreadable. Plan §1 has the
one-line improvement I would try first and the full re-render fix.

### `ui/app.js` — a `print` section

`printDocument()`: guards on `currentFile` (a blank page, or the "Select a
markdown file" placeholder, is not something to hand to a printer), then
`invoke("print_document")`, toasting the error if it fails. Nothing is closed or
toggled first — the print sheet hides the panels itself, so the output does not
depend on the chrome state. One wiring line in `wireUi()`.

### `src-tauri/src/main.rs` — `print_document`

```
app.get_webview_window("main") -> .print() -> map_err(to_string)
```

Six lines plus a doc comment recording the WKWebView finding, and one entry in
`generate_handler!`. **No new module, no logic, nothing that belongs in the
library crate** — the whole body is a Tauri call, which is exactly what `main.rs`
is for per CLAUDE.md.

## Files touched

- `ui/index.html` — `#btn-print` in `#tb-actions`; new `<style id="print-css">`.
- `ui/app.js` — new `print / export to PDF` section; one line in `wireUi()`.
- `src-tauri/src/main.rs` — `print_document` command, registered.
- `README.md` — a **Print / save as PDF** bullet, and the **Top bar** bullet now
  names the icon.
- `docs/plans/md-to-pdf-export.md` — syntax colour, the annotations appendix, and
  the keybind/menu options.

No new dependency, no `Cargo.toml` change, no CSP change, no config key, no
keybind, no `KEY_ACTIONS` row. `perf/harness/` untouched — nothing there asserts
on the titlebar's contents and the `rows === 17` count is unaffected.

## Verification

- **Build gate: `GATE PASS`** — `cargo build --lib` clean, bin at exactly the 5
  pre-existing macOS-gating errors.
- **`main.rs` *is* compile-checked here, contrary to the batch's working
  assumption.** Verified by differential: replacing `.print()` with a
  nonexistent method took the bin error count from 5 to 7, so rustc type-checks
  `main.rs` past the pre-existing resolution errors. `print_document`'s
  signature, the `get_webview_window`/`print` calls and the `generate_handler!`
  registration are all genuinely verified, not eyeballed. (This is worth knowing
  for future batches.)
- `node --check ui/app.js` passes.
- The print CSS block was parsed for balanced braces and unterminated comments.
  That is *all* it was checked for.
- **Nothing has been printed, or rendered.** No Tauri run, no WebKit, no
  Chromium (Playwright's download is proxy-blocked here), so no page from this
  stylesheet has ever existed.

## Left open

- **The printed page has not been seen.** Most likely first-sight tuning, all
  single values in `#print-css`: the 160mm measure and 11pt size, the 16mm
  `@page` margin, and whether the appended `(https://…)` after external links
  reads as useful or as clutter (one rule, deletable).
- **Whether WebKit applies `@media print` to `printOperationWithPrintInfo:`.**
  It should — the print operation renders in paged media — but it is the single
  assumption the whole feature stands on, and it is unverifiable here. If the
  first print comes out looking like the screen, that is where to look, and the
  fallback is `window.print()` in a debug build to compare.
- **`print-color-adjust` is not set**, so the code-block slab and the table
  header tint will only print if the reader ticks "background graphics". Borders
  carry the structure either way; left off as the polite default.
- **Printing from view mode is impossible** — it hides the titlebar and with it
  the only trigger. Esc first. The File → Print menu item in plan §3 is the fix.
- **Syntax colour, the annotations appendix, and a keybind** are designed but not
  built — `docs/plans/md-to-pdf-export.md`.

perf not run - pending manual check on the author's machine
