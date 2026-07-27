# Plan — jump back / jump forward (navigation history)

> **Built, 2026-07-27.** Keys are `Ctrl+[` / `Ctrl+]` — §2 option 3, chosen by
> the author over the recommended `Alt+Left`/`Alt+Right` because they echo the
> bare `]`/`[` file-step one modifier away. §3's broad rule was taken as
> written. Two deviations from §4, both argued in `ui/app.js`: the cross-file
> push lives inside `openFile` while in-document pushes sit at their two call
> sites, because a single push inside `scrollToFragment` double-counts a
> `other.md#section` link and records an intermediate frame nobody read; and
> marks were cut to one slot in the same pass, so the two features now share
> the `{ path, top }` frame and `restoreFrame` exactly as §7 anticipated.

Source idea: `ideas/done/file-and-section-links.md`. Written during the overnight
batch on `claude/overnight-ideas-2026-07-27`.

The idea bundles three things. Two of them are **already done** and are not in
this plan:

| Piece | Status |
| --- | --- |
| Cross-file relative `.md` links | Shipped before this batch (`interceptLinks`). |
| Heading ids | Shipped in commit `790a883` (`markdown::Slugger`). |
| Same-document `#anchor` jumps | Work as of `790a883`. |
| Cross-file `other.md#section` jumps | **Implemented in this batch** — see `docs/idea-logs/file-and-section-links.md`. |
| Path containment on file links | **Implemented in this batch** (`insideRepo`). |
| **Jump back / jump forward** | **Not built. This document.** |

Jump-back was held back because it is the one cross-cutting piece: it needs a
new keymap entry (five files, one of which cannot be exercised on Linux), it
touches every navigation entry point, and — see §2 — the two keys the idea
proposes are **both already bound to something else**, which is a product
decision rather than a coding one.

---

## 1. What exists to build on

`openFile(path)` (`ui/app.js`, ~line 366) is a genuine funnel — three lines,
and *every* navigation in the app goes through it:

| Caller | Site | Should it push? |
| --- | --- | --- |
| Startup / CLI path | `init`, ~line 129 | No — nothing to go back to. |
| File-tree click | `paintTree`'s `name.onclick`, ~line 337 | See §3. |
| Palette click | ~line 874 | See §3. |
| Palette `Enter` | ~line 1061 | See §3. |
| **Link click** | `interceptLinks`, ~line 445 | **Yes** — this is the feature. |
| File → Open (new repo) | ~line 986 | No — and the stack must be *cleared*. |

Scroll position is `scrollEl.scrollTop` (`const scrollEl = $("content-scroll")`).
`renderCurrent` already restores it via its `preserveScroll` flag, so the
"restore where I was" half of a pop is a pattern the file already has.

In-document `#anchor` jumps do **not** go through `openFile` — after this
batch they go through `scrollToFragment(frag)`, which is the single place an
intra-document jump can be intercepted.

## 2. The keybind problem — decide this first

The idea proposes vim's jumplist keys. Both are taken:

- **`Ctrl+O`** — `Keymap::toggle_stack` (`src-tauri/src/config.rs`).
- **`Ctrl+I`** — `Keymap::toggle_outline`, claimed in commit `790a883`.

That is not a detail to route around silently; four options, in the order I'd
rank them:

1. **`Alt+Left` / `Alt+Right`.** Browser/IDE back-forward. No collision, reads
   as "navigation" to anyone who has used a browser, and does not pretend to be
   vim when the rest of the app is not modal. **Recommended default.**
2. **Take `Ctrl+O` back for jump-back and rehome `toggle_stack`.** Most faithful
   to vim, and jump-back is arguably the more frequent action — but it moves a
   key the author already has in muscle memory, so it needs the author's say-so.
3. **`Ctrl+[` / `Ctrl+]`.** Free, vaguely vim-ish (`Ctrl+[` is `Esc` in a
   terminal, which is a real hazard if dreamd ever grows a terminal surface).
4. **Bind back only, skip forward.** Halves the surface. Forward is the much
   rarer motion, and it is the one that needs the "clear the forward stack on a
   new navigation" rule below.

Whichever is chosen, both are rebindable from the settings panel, so this
decides the *default* only.

## 3. The idea's open question, answered

> Does a plain scroll-to-heading (no link involved) also push onto the
> jump-back stack, or only actual link clicks?

**Recommendation: push on any jump that moves you somewhere you did not scroll
to yourself.** Concretely, push a frame when — and only when —

- a link click navigates to another file (`interceptLinks`), **or**
- a link click or an outline-panel click jumps within the document
  (`scrollToFragment`, `buildOutline`'s `.oi` handler), **or**
- a tree or palette click opens a different file.

Do **not** push on: plain wheel/keyboard scrolling, `file-changed` re-renders,
theme or appearance switches, or `openFile(currentFile)` (a no-op move).

Rationale: the frames the user actually wants are "I was reading X and
something teleported me". Manual scrolling is not a teleport — vim's jumplist
excludes `j`/`k` for exactly this reason, and a stack that records passive
scrolling fills with junk within a minute. Tree and palette clicks *are*
teleports, and the idea file leaves them optional; including them makes the
feature "undo my last jump" rather than "undo my last link click", which is
the simpler thing to explain and the one a reader will expect. Outline-panel
clicks are the same motion as an `#anchor` link and belong in for consistency.

The whole rule lands as: **one `pushJump()` call at the top of `openFile`
guarded on `path !== currentFile`, plus one inside `scrollToFragment` guarded
on "the target was found"** — plus explicit suppression on the pop path. Two
call sites, which is why the recommendation is the broad rule and not the
narrow one; the narrow one needs a flag threaded through `openFile` from six
callers.

## 4. Design

```js
// Navigation history. In-memory only and dies with the process, like every
// other piece of session state (tenet 2) — never serialized to disk.
const jumpBack = [];     // [{ file, scroll }], newest last
const jumpFwd  = [];
const JUMP_MAX = 64;     // ring: drop the oldest, never grow without bound
let jumping = false;     // true while a pop is in flight; suppresses pushes
```

Store `{ file, scroll }`, **not** `{ file, anchor }`. A scroll offset restores
the exact reading position including mid-section; an anchor only ever gets you
back to the top of a heading, and a document with no headings gets nothing.
`renderCurrent`'s `preserveScroll` already proves the offset survives a
re-render of the same document.

Four operations:

- `pushJump()` — capture `{ file: currentFile, scroll: scrollEl.scrollTop }`,
  no-op if `jumping` or `currentFile` is null; push, trim to `JUMP_MAX` from
  the front, and **clear `jumpFwd`** (a new jump invalidates the forward
  branch — standard undo-stack semantics, and the reason forward is the
  fiddlier half).
- `jumpBackward()` — pop `jumpBack`; push the *current* position onto
  `jumpFwd`; `restore(frame)`.
- `jumpForward()` — mirror image.
- `restore({ file, scroll })` — set `jumping = true`, then: if `file` is
  already `currentFile`, just set `scrollEl.scrollTop`; otherwise
  `await openFile(file)` and set the offset after it resolves (`openFile`
  resets scroll to 0 last, same ordering constraint the cross-file fragment
  jump already handles). `finally { jumping = false }`.

### Failure modes that need an answer before coding

- **The file is gone** (deleted through the file menu, or removed on disk while
  you were away). `render_markdown` fails and `renderCurrent` paints an error
  block rather than throwing, so a naive pop leaves you staring at an error
  with the frame consumed. Fix: on the `file-removed` watcher event, filter
  both stacks — `jumpBack = jumpBack.filter(f => f.file !== path)`. Cheap, and
  it also covers the `doDeleteFile` path.
- **The repo root moved** (File → Open, ~line 986). Every frame is a path into
  the old root. **Clear both stacks** there.
- **The document got shorter.** A stored offset can exceed the new
  `scrollHeight`; the browser clamps, so this needs no code — but it means a
  restore is approximate after an edit, which is correct behaviour and worth a
  line in the README rather than an attempt to re-anchor.
- **Empty stack.** `toast("No earlier position")` — silence reads as a broken
  keybind.

## 5. Files to touch

| File | Change |
| --- | --- |
| `src-tauri/src/config.rs` | `Keymap::jump_back` (+ `jump_forward`) fields, doc comments, `Default` entries. Library crate — fully compile-checked. |
| `ui/app.js` | The stack + four functions; `pushJump()` in `openFile` and `scrollToFragment`; two `matchCombo` arms in the keydown chain (~line 1146); two `KEY_ACTIONS` rows (~line 1264); stack invalidation in the `file-removed` listener and the File → Open path. |
| `README.md` | Keybind table + the `[keymap]` sample block. |
| `perf/harness/lib/fixtures.mjs` | Mocked keymaps must gain the new fields. |
| `perf/harness/ui-check.mjs` | The `every action gets a row` count goes 11 → 12 (or 13 with forward). |

**`src-tauri/src/main.rs` is not touched** — `get_keymap` returns `Keymap`
wholesale, so new fields flow through with no handler change. That is what
keeps this a safe change to make on a machine where the bin target cannot
compile, and it is worth preserving: if a step in the implementation seems to
need a new `#[tauri::command]`, that is a signal the design has drifted.

## 6. Order of work

1. Get the author's answer on §2 (keys) and confirm §3 (what pushes). Both are
   product calls; everything after is mechanical.
2. `config.rs` fields + defaults. Run the build gate and
   `cargo run --example config_check` (it covers keymap layering).
3. `ui/app.js`: stack, `pushJump`, `restore`, the two handlers. Verify by hand
   in `cargo tauri dev`: link → back → forward, tree → back, back into a
   deleted file, back across a File → Open.
4. README + harness fixtures.
5. `/perf-quick`. The only hot-path addition is one `pushJump()` per navigation
   — an object literal and an array push — so a move would be surprising, but
   navigation is the loop this feature lives in and it should be measured.

## 7. Explicitly out of scope

- **Persisting the stack across runs.** Tenet 2. It dies with the process.
- **A visible history UI** (a dropdown of recent positions). Bigger product
  surface; the keybind is the whole feature.
- **Merging with `ideas/done/vim-marks-bookmark-jump.md`.** The idea file already
  argues these stay separate, and it is right: automatic per-navigation history
  and explicit `m{letter}` marks share a frame *shape* and nothing else. If
  both get built, share the `{ file, scroll }` frame and the `restore()`
  helper; do not share the stack.
