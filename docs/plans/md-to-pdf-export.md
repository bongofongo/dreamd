# Plan — the rest of PDF export

Source idea: `ideas/done/md-to-pdf-export.md`. Written during the overnight batch on
`claude/overnight-ideas-2026-07-27`, alongside the half that shipped.

**The content-only export is built.** Printer icon in the top bar →
`print_document` → OS print dialog → Save as PDF, with an `@media print`
stylesheet (`#print-css` in `ui/index.html`) deciding what the page looks like.
See `docs/idea-logs/md-to-pdf-export.md`.

This document is the three things deliberately left out of that pass, in the
order I would do them:

| Piece | Why it was held back |
| --- | --- |
| §1 Syntax colour on paper | Needs a second render round trip on the print path. |
| §2 Highlights / annotations as an appendix | The idea's open question; answered "not yet", not "no". |
| §3 A keybind or a menu item | Five files, and the obvious key is taken. |

Nothing below is urgent. The shipped half is the whole of what the idea asked
for; each of these is a refinement that needs the printed page to have been
*seen* first, which nothing in this container could do.

---

## 1. Syntax colour on paper

**What happens today.** `#content pre span { color: inherit !important; }`, so
fenced code prints in the body colour with syntect's bold and italic surviving
and its hues thrown away.

**Why.** syntect writes token colours as *inline styles* on every span, chosen
by `--syntax-theme` for the reader's current appearance. Under any dark palette
those are pale pastels; on a white page they are unreadable. Author `!important`
is the only thing in CSS that outranks an inline style, so the choice is binary
from the stylesheet alone: keep every theme's colours, or keep none. Losing
colour on a light palette is a cosmetic cost; keeping it on a dark one produces
a page you cannot read. The current rule takes the safe side.

**The real fix, if it is wanted.** Re-render the document through the *light*
half of the palette immediately before printing, so the inline colours syntect
bakes in are already paper-appropriate:

1. `print_document` grows a `scheme: Scheme` argument, or a sibling command
   `render_markdown_for(path, scheme)` is added. `theme::custom_property` already
   takes a `Scheme` and `mode_slice` already extracts one appearance's blocks, so
   reading the light `--syntax-theme` out of the active palette is a call that
   exists, not new logic.
2. `markdown::render` picks the syntect theme from that value rather than from
   the session's resolved mode.
3. `printDocument()` swaps `#content.innerHTML` for the light render, prints,
   and swaps it back.

**Three reasons this was not worth doing blind:**

- Step 3 replaces the DOM under the highlights. `applyHighlights` would have to
  re-run on the way back, or the marks are gone until the next render — and
  every `mark.hl` in the light render is a fresh anchoring pass on the print
  path, which is exactly the wrong place for the app's most expensive operation.
- The swap must happen *before* the print operation captures the page and be
  undone *after*. `WebviewWindow::print` is modal-to-window and returns
  immediately on macOS (`runOperationModalForWindow:…`), so "after" is a
  callback that does not exist yet. Getting this wrong leaves the reader looking
  at a light-theme document in a dark app.
- It is a second full render of the open document on every print. For the
  documents dreamd is pointed at that is the single most expensive thing the app
  does, and this container cannot run a perf tier to find out what it costs.

**Cheaper 80% alternative, if the above reads as too much:** keep the current
rule but scope it to dark appearance only —
`:root[data-mode="dark"] #content pre span { color: inherit !important; }` — so
a reader on a light palette keeps their syntax colours and a reader on a dark
one still gets a legible page. One line, no Rust, no re-render. The only thing
it gets wrong is a palette whose light block names a *dark* `--syntax-theme`,
which is a palette bug the `theme_check` example already warns about. **This is
the one I would try first.**

## 2. Highlights and annotations on the page

The idea's open question, verbatim: *"Print just the content, or fold in the
highlight/annotation stack somehow (e.g. highlights rendered as marginalia, or
the stack as an appendix)?"*

**Answered content-only for the shipped pass**, and the reasoning is in the
`#print-css` comment: highlights are session state that dies with the process
(tenet 2), a PDF is durable, and a *stale* highlight's red says something about
anchoring that means nothing on paper. Content-only also makes the export
reproducible — the same file gives the same PDF regardless of what this session
marked up.

That is an argument for the *default*, not against the feature. Two shapes, if
it comes back:

### 2a. Marginalia — the wrong one, and worth writing down why

Render each highlight's annotation in the margin beside its quote. Attractive on
screen, and a trap in print:

- There is no margin to put it in. The print sheet sets a 160mm measure inside a
  16mm page margin precisely so the text reads at ~80 characters; a marginal
  note needs another ~45mm, which either shrinks the measure below comfortable
  or overflows the page box.
- CSS has no way to keep a floated note beside its anchor across a page break.
  The note lands on the page its float was resolved on, and the quote can
  paginate away from it. Every real implementation of this (`::footnote`, CSS
  Paged Media Level 3) is in browsers dreamd does not run on.

### 2b. An appendix — the one I would build

After the document, a page break, then a heading and one entry per stacked pair:
the quote, the annotation, and the source line range. `annotations::Store`
already holds exactly this and `stack_query_text` already assembles a very
similar document for `send.rs`, so the content is a solved problem.

Shape:

- A checkbox in a small popover on the print button — *"Include annotations"* —
  defaulting off. Not a config key: it is a per-export choice, and `config.toml`
  is for preferences that persist (tenet 2 again).
- The appendix is built into a `<section id="print-appendix">` that lives in the
  DOM at all times and is `display: none` outside `@media print`, so nothing on
  screen changes and the print sheet stays the only thing that decides paper.
  Populate it from `get_stack` on the print path; empty it after.
- `#print-appendix { break-before: page; }` and `display: block` inside the
  print block, gated on a class the button's checkbox sets.
- Only *this file's* highlights, or the whole cross-file stack? The stack spans
  every file opened in the session, and an appendix listing quotes from three
  other documents attached to this one is confusing. Filter to `currentFile`.

Note this also settles the marks: if the appendix is on, `mark.hl` should
probably print as a light underline (not a fill) so the appendix's entries can be
found in the body. That is a second CSS rule under the same class, not a
redesign.

## 3. A keybind or a menu item

The shipped trigger is a titlebar button and nothing else, following
`#btn-hl-mode`, which is the existing precedent for a titlebar action with no key
of its own. Printing is rare and deliberate; it does not obviously earn a key.

**If a key is wanted, `Ctrl+P` is not free.** It is `Keymap::palette_prev`. The
collision is *functionally* harmless — `palette_prev` is only read inside the
palette input's own `onkeydown`, and the global handler bails while the palette
overlay is open — but `comboClashes()` in the settings panel compares raw combo
strings across all of `KEY_ACTIONS` and would paint both rows red. Shipping a
default binding that the app's own settings panel flags as broken is not
acceptable. Options, ranked:

1. **`Ctrl+Shift+P`.** Free, adjacent to the convention, no clash. Recommended.
2. **A File → Print menu item with `CmdOrCtrl+P`.** The most macOS-native answer
   by some distance, and it sidesteps `matchCombo` entirely: `menu.rs` already
   uses `Cmd+O` alongside a `Ctrl+O` binding for exactly this reason (see its
   header comment). It is also the *only* way to print while in view mode, which
   hides the titlebar and with it the button. Costs a `MenuItem::with_id` in
   `menu.rs`, an id constant, and an `on_menu_event` arm in `main.rs` that emits
   an event the frontend listens for — or calls `print_document`'s body directly,
   since it already only needs the `AppHandle`. **Do this one if either.**
3. Take `Ctrl+P` and rehome `palette_prev`. Not worth moving a key in the
   author's muscle memory for an action used once a month.

Whichever: the full established pattern is a field on `Keymap` +
`Keymap::default()` in `src-tauri/src/config.rs`, the `keymap` literal at the top
of `ui/app.js`, a `matchCombo` arm in `wireKeys`, a `KEY_ACTIONS` row, a
`data-tip-key` on the button, the `rows === N` assertion in
`perf/harness/ui-check.mjs` (17 at the time of writing, and the fixture at
`perf/harness/lib/fixtures.mjs` plus the second literal near line 361), and a
README line.

---

## What is *not* worth doing

**Adding a PDF crate.** The idea file recommends against it and the shipped
implementation is the reason why: the entire feature is a stylesheet and a
six-line command, it produces a PDF through a path the OS already maintains, and
it added zero dependencies. `printpdf`/`weasyprint`-shaped crates would add
megabytes to a binary whose icon array order is documented in CLAUDE.md because
4MB mattered, and would have to reimplement pagination, fonts and syntax colour
that WebKit already does.

**Steering the save dialog away from the repo.** The idea file leans toward "the
user explicitly chose where to put it" and that is right. Tenet 1 is about the
app not mutating repo content on its own; a user aiming their own OS save dialog
at a folder is not dreamd writing to it, and dreamd could not intercept the
choice anyway without replacing the native dialog — which would be a worse
product for a purity that the tenet does not actually demand.
