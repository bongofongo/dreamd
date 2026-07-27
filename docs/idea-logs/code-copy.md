# Idea log — copy button on code blocks

Source idea: `ideas/code-copy.md` ("code containers need to have a copy button
somewhere - probably the standard top right"). Overnight batch, branch
`claude/overnight-ideas-2026-07-27`.

**Decision: implemented, not planned.** The whole change is frontend, and no
Rust at all — `copy_to_clipboard` is already a registered command
(`src-tauri/src/main.rs:464`, `send::copy_clipboard`, arboard text-only), used
today by "Copy path" and by the stack copy. So `main.rs` was **not touched**
and the container's missing compile check never came into play.

## Files touched

- `ui/app.js` — `decorateCodeBlocks()` / `copyCodeBlock()`, plus the call and a
  `d:decorate_code` perf span in `renderCurrent`.
- `ui/index.html` — `.code-block` / `button.code-copy` rules in the chrome
  `<style>`, next to `mark.hl`.

Nothing in `src-tauri/`, and no new `#[tauri::command]`.

## What was done

`decorateCodeBlocks()` runs after each render, in the same slot as
`interceptLinks()` and before `applyHighlights()`. For every `#content pre` it
wraps the block in a `div.code-block` and appends a `button.icon.code-copy`
positioned top right, revealed on hover (and on `:focus-visible`). Clicking it
sends `pre`'s text to `copy_to_clipboard`, swaps the icon to a tick for 1.4s
and toasts "Code copied".

Five decisions worth keeping:

1. **Post-render DOM decoration, not markup from `markdown::render`.** The
   button is chrome, not document content; keeping it out of the render pass
   keeps it out of everything that reads the rendered HTML, and out of the
   Rust build entirely.
2. **Zero text nodes in the button.** This is the real risk in the idea.
   Highlight anchoring reads *rendered DOM text* — `getSelection().toString()`
   for a new quote, and the text-node walks in `applyHighlights` /
   `selectionContext` for placing and re-placing one. A "Copy" text node inside
   `#content` would be picked up by a drag across the block and land inside the
   reader's quote, which `markdown::locate` would then fail (or mis-) anchor.
   So the button carries two inline SVGs and no text: the wording lives on
   `aria-label` and `data-tip` (attributes, invisible to text walks), the
   "copied" state is a `data-copied` attribute that CSS swaps icons on, and the
   confirmation wording goes to the existing `#toast`, which sits outside
   `#content`. `user-select: none` is a second, independent layer of the same
   guard.
3. **Sibling of the `<pre>`, not a child.** `#content pre` has
   `overflow-x: auto`, so a child button would slide out of view on a wide
   block and widen the scroll area. Being outside also keeps it off the path a
   reader drags across when selecting code.
4. **`textContent`, never re-parsed markup** (tenet 4). syntect's `<span>`s and
   any `mark.hl` the reader has placed are markup that would otherwise need
   stripping by hand. `textContent` is the code as read, and marks contribute
   nothing to it. A leading newline (syntect emits one after `<pre …>`) and
   trailing whitespace are trimmed. Note syntect's output has **no `<code>`
   element** — only `fallback_code` emits one — hence
   `pre.querySelector("code") || pre`.
5. **Existing Rust clipboard path, not `navigator.clipboard`.** Same delivery
   as every other copy in the app, no dependence on webview clipboard
   permissions or a user-activation gate, and no new command to review by eye.

Two smaller guards: the click handler `stopPropagation`s so it cannot reach
`#content`'s `mark.hl` click listener, and a `mouseup` handler `stopPropagation`s
so that in highlight mode a leftover selection elsewhere in the document cannot
turn a copy click into an annotation modal. Re-render survival is free —
`renderCurrent` assigns `contentEl.innerHTML` wholesale, so the old wrappers and
buttons are discarded and `decorateCodeBlocks()` rebuilds them; the function is
also idempotent (it skips a `pre` already inside a `.code-block`), so nothing
breaks if it is ever called twice.

Tenets: nothing written (1), nothing persisted (2), no shell involved (3),
no markup re-parsed (4), all colours are palette vars with fallbacks (5).

## Left open

- **Not run in a real webview.** `node --check` passes on `app.js`; the
  container has no Chromium (`ui-check.mjs` is proxy-blocked) and no macOS, so
  the button has not been seen on screen. Worth eyeballing: contrast of
  `background: var(--bg)` against the code slab across the bundled palettes,
  particularly any theme setting `--code-bg: transparent`.
- The button is a Tab stop per code block. That is the accessible default and
  matches `.oi` in the outline panel, but on a code-heavy document it is a lot
  of stops; `tabindex="-1"` is the one-line alternative if it annoys.
- No keybinding for "copy the code block under the cursor". Deliberate — the
  keymap is a flat single-combo table and this needs a notion of "current
  block".
- A new `d:decorate_code` perf phase now appears in `perf` builds. It has no
  baseline entry.

perf not run - pending manual check on the author's machine
