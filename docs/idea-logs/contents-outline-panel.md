# Idea log — contents / outline panel

Source idea: `ideas/contents-outline-panel.md`. Overnight batch, branch
`claude/overnight-ideas-2026-07-27`.

**Decision: implemented, not planned.** The idea splits into three pieces —
heading ids in the render pass, a heading list, and a UI surface — and each one
turned out small and self-contained. The Rust half is confined to
`markdown.rs` (library crate, so fully compile-checked) plus one `Keymap`
field. The heading list needs no Rust at all: the ids are in the rendered DOM,
so the panel is one `querySelectorAll`. **No `#[tauri::command]` was added and
`main.rs` was not touched**, which was the deciding factor — `get_keymap`
already returns `Keymap` wholesale, so a new field flows through with no
handler change.

## What was done

### Heading ids (`src-tauri/src/markdown.rs`)

`render_with` now assigns every heading an `id`. pulldown-cmark's HTML writer
already emits and escapes `Tag::Heading { id }` when it is `Some`; it was
always `None` because `ENABLE_HEADING_ATTRIBUTES` is off. The event loop gained
two arms: `Start(Tag::Heading)` records the event index and opens a text
accumulator, `End(TagEnd::Heading)` slugs the accumulated text and patches the
id into the start event in place. Headings do not nest, so one slot suffices.

Heading text is accumulated from `Event::Text`, `Event::Code` (inline code),
`Event::Html` / `InlineHtml` (which dreamd renders as escaped *text*, so it is
text the reader sees), and `SoftBreak` / `HardBreak` → a space, which is how a
multi-line setext heading reads.

**Slug scheme — `markdown::Slugger`, public and reusable.** GitHub's, because
a `[jump](#some-heading)` written in a repo's own markdown was written against
GitHub's:

1. trim, then `str::to_lowercase` (Unicode-aware — `Ünïcödé Ça Va` →
   `ünïcödé-ça-va`);
2. keep `char::is_alphanumeric()`, `-` and `_`; each whitespace char becomes a
   `-`; everything else is dropped. Runs are **not** collapsed, matching
   `github-slugger` — `Setup & Config` → `setup--config`;
3. an empty result becomes `section` (const `EMPTY_SLUG`) — e.g. `## ***`.

**Dedupe rule.** `Slugger::reserve` keeps a `HashSet` of everything minted for
the document. The first occurrence keeps the bare slug; each later one gets
the first free `base-1`, `base-2`, … . Unlike GitHub, the *numbered candidate
is itself checked*, so a document with `## A 1` and `## A` three times comes
out `a-1`, `a`, `a-2`, `a-3` — every id distinct, guaranteed, because ids are
what section anchoring will key on. `Slugger::reserve` is also called directly
for an id the source stated itself (dead today, since the pulldown option is
off, but it means an explicit id is never silently overwritten and never
double-claimed).

Slugs are deterministic: they depend only on the heading text and the headings
before it in the same document.

### Contents panel (`ui/index.html`, `ui/app.js`)

- `#outline-panel`: same visual language as `#stack-panel`, but anchored to
  the **left** edge of `#main-wrap` so both can be open at once instead of
  fighting for the right-hand slot. `#btn-outline` is the new leftmost
  titlebar icon.
- `buildOutline()` walks `contentEl.querySelectorAll("h1…h6")`, builds one
  `DocumentFragment` of `.oi` buttons indented by level, and inserts once.
  Entry text is `h.textContent` (never `innerHTML`), so inline markup and any
  `mark.hl` inside a heading come out as the text the reader sees, and nothing
  needs escaping. Clicking scrolls **the element** into view rather than
  resolving its id — a jump that cannot miss.
- `#content :is(h1…h6) { scroll-margin-top: 16px }` so a jump does not park the
  heading flush against the top edge. This also improves in-document
  `#anchor` links.

**The idea's open question (live-update vs rebuild-on-open) is answered as
both.** `refreshOutline()` runs at the end of every `renderCurrent`, plus on
the render-error, file-removed and repo-changed paths that set `#content`
directly. It rebuilds immediately *if the panel is open*, and otherwise only
sets `outlineDirty`, which `toggleOutline` consumes on the next open. So an
open panel tracks `file-changed` the way every other surface does, and a
closed one costs a boolean per render rather than a DOM walk nobody is looking
at the outline of.

### Keybind

New `Keymap::toggle_outline`, default **`Ctrl+I`** ("index"). `Ctrl+B` — the
obvious sidebar-toggle key — was deliberately left free, because
`ideas/hide-file-tree-keybind.md` already names it. Wired into the global
keydown chain and into the settings panel's `KEY_ACTIONS` so it is
user-rebindable like every other shortcut.

### Bug fixed on the way

`interceptLinks`'s `#anchor` branch did `contentEl.querySelector(href)`. That
was harmless while nothing had an id; with heading ids live it becomes a real
bug, because a slug like `1-intro` (from `## 1. Intro`) is a **valid id** and
an **invalid CSS id selector** — `querySelector("#1-intro")` *throws* rather
than returning null, and the throw would have escaped the click handler. It
now scans `contentEl.querySelectorAll("[id]")` for a matching `el.id`, which
also (a) needs no escaping, (b) percent-decodes the fragment, and (c) scopes
the lookup to `#content`, so a document containing `[x](#content)` or
`[x](#tree)` can no longer scroll the app's own chrome into view.

## Files touched

- `src-tauri/src/markdown.rs` — heading ids, `pub struct Slugger`.
- `src-tauri/src/config.rs` — `Keymap::toggle_outline` + default.
- `ui/index.html` — `#outline-panel` markup, `#btn-outline`, panel CSS,
  `scroll-margin-top` on headings.
- `ui/app.js` — outline panel section, `refreshOutline` calls, keybind,
  `KEY_ACTIONS` entry, `#anchor` link fix.
- `README.md` — keybind table, `[keymap]` sample, Usage bullet.
- `perf/harness/ui-check.mjs`, `perf/harness/lib/fixtures.mjs` — the mocked
  keymaps had to gain `toggle_outline`, and `every action gets a row` had to go
  from 10 to 11 (10 `KEY_ACTIONS` + the `quick_highlight` checkbox row).

**`src-tauri/src/main.rs` was NOT touched**, so nothing in this change is
missing a compile check for that reason.

## Verification

- The overnight build gate printed **GATE PASS** (`cargo build --lib` clean;
  bin still at exactly the 5 pre-existing macOS-gating errors).
- `cargo run --release --example locate_check` — exit 0, 611 fixtures, 0 wrong
  / 0 moved / 0 unresolved / 0 disagreements. Heading ids do not change
  rendered *text*, so highlight anchoring is untouched, and this confirms it.
- `cargo run --example config_check` — 34 passed, 0 failed.
- Slug behaviour was checked against a throwaway example (since removed)
  covering repeats, `&`, inline code, setext, unicode, punctuation-only
  headings, and the `## A 1` / `## A`×3 collision case. Output as documented
  above.
- `node --check ui/app.js` passes.

## Left open

- **`perf/harness/ui-check.mjs` was edited but could not be run.** Playwright's
  Chromium download is blocked by this container's proxy (403 `host not
  permitted`), so `npm run setup` fails. The two fixture edits and the
  `rows === 11` bump are by-eye. Run it on the author's machine.
- **The panel was never rendered.** No Tauri/WebKitGTK run and no Chromium, so
  the CSS and the click-to-jump path are unexercised. Everything about them is
  conventional, but they have not been seen.
- **No scroll-spy / active-heading marker.** Deliberate: "how far through the
  doc you are" is `ideas/reading-progress-indicator.md`, which the idea file
  explicitly calls a separate concept — and a scroll listener is exactly the
  thing `ui/index.html`'s `content-visibility` note warns about. If it is
  wanted later, `IntersectionObserver` over the headings is the cheap way in,
  and it should be measured.
- `Ctrl+I` is a guess at a free key. Nothing else binds it, but the author may
  prefer something else; it is rebindable from the settings panel either way.

## For the next agent (`ideas/file-and-section-links.md`)

**Heading ids now exist**, generated as described above.

- Reuse `dreamd::markdown::Slugger` — it is `pub`, `Default`, and has
  `slug(&mut self, text: &str)` plus `reserve(&mut self, base: &str)`. To
  resolve a cross-file `other.md#some-section`, run a fresh `Slugger` over the
  target document's headings in document order and match; do **not**
  reimplement the scheme, or the two will drift.
- The id is minted from the heading's *rendered text*, not its source, so
  anything that reconstructs a slug must render (or at least strip markdown)
  first.
- Frontend-side, `interceptLinks` already handles same-document `#fragment`
  links against these ids (see the bug note above). A cross-file
  `path.md#fragment` link currently drops the fragment —
  `target = normalizePath(base + href.split("#")[0])` — so that is the hook to
  extend: `openFile(target)` and then scroll to the fragment once the render
  has landed.
- `#content :is(h1…h6) { scroll-margin-top: 16px }` is already in
  `ui/index.html`, so any jump you add lands with breathing room for free.

perf not run - pending manual check on the author's machine
