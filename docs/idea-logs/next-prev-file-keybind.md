# Idea log: next / previous file keybind

Idea file: `ideas/next-prev-file-keybind.md`. **Implemented in full — no plan
doc.** Every part the idea asked for landed: the flattened ordering, the
`Keymap` entries, the settings-panel actions, the keydown branches, and a
decision on the ends of the list. There was nothing left over worth planning.

## Why implement rather than plan

This is the cheapest shape in the batch. It adds no Rust *logic* at all — two
`String` fields and two default values — no new `#[tauri::command]`, no IPC, no
`main.rs` change, and no new state. The one real design question (where the
ordering comes from) had an answer that removed work rather than adding it. It
is squarely the "doesn't pose great risk and is one of the easier ones" side of
the user's criteria, and it follows the four preceding keybind commits exactly.

## The ordering: read the rendered tree, don't re-derive it

The idea proposed flattening `FileNode` depth-first and warned "don't invent a
second ordering." The strongest way to honour that warning turned out to be not
writing a flattener at all.

`renderNode` already emits `.tree-item.file` elements depth-first into `#tree`,
so **document order in the sidebar DOM is already the flattened depth-first
order**, complete with `data-path` on every entry. `stepFile` reads it back with
one `querySelectorAll`. That means:

- There is only ever *one* ordering in the system. A Rust flattener would be a
  second derivation of the same fact, free to drift from what the sidebar shows
  the moment either side changes.
- It follows every repaint for free — `rebuild_index`, the watcher's
  `file-added` / `file-removed`, and File ▸ Open moving the root all go through
  `paintTree`, and the next keypress sees the new order with no invalidation
  logic anywhere.
- No IPC round trip per keystroke, and no new command in `main.rs` — which the
  overnight gate could not have compile-checked anyway.

The cost is that the binding does nothing until the tree has painted. `loadTree`
is deliberately off the boot critical path, so on a single-file launch there is a
brief window where `]` is a no-op. That is the correct behaviour regardless:
there is no list to step through yet.

## Decisions

**Wrap around, don't stop.** `movePalette` already wraps
(`(paletteSel + d + len) % len`), so wrapping is the in-repo precedent for list
navigation. More to the point, there is no affordance for "that was the last
file" — stopping would be a silent no-op indistinguishable from a dead key or an
unbound one. A `toast()` on the boundary was the alternative and was rejected as
noise on a key meant to be pressed repeatedly.

**Collapsed directories are not skipped.** Collapsing sets `display:none` on the
children and leaves them in the DOM, so `querySelectorAll` returns them either
way and skipping would have been the extra work, not the default. It would also
be wrong: the point of the binding is to move *without touching the tree*, which
may be collapsed entirely (`nav-collapsed`) or hidden outright by view mode.
Filtering on visibility would make files unreachable by keyboard depending on
state the user is not looking at — and in view mode, cannot look at.

**`]` / `[`, as the idea suggested.** They are the near-universal next/prev
convention and both were free. They are the first bare-*punctuation* defaults in
the keymap, which is a smaller claim than it sounds: the dispatch sits below both
the overlay guard and the `isEditable` guard, so the annotation textarea, the
palette input and the settings fields never see them. That is a different
situation from `quick_highlight`'s bare `h`, which is opt-in because it collides
with vim-style letter expectations — brackets carry no such load in a reader.
Both are rebindable, and `comboFromEvent` encodes them cleanly (`e.key` is `"]"`,
no `+` to confuse the splitter).

**`scrollIntoView({ block: "nearest" })` on the target row.** Without it the
active row walks off the top of the sidebar after a few presses, since `#tree`
scrolls independently of `#content-scroll`. Scoped to `stepFile` rather than
folded into `markActiveInTree`, so opening a file by click or palette keeps its
current behaviour exactly.

**Index lookup via `activeTreeItem`, not a scan.** `[].indexOf.call(files,
activeTreeItem)` reuses the reference the tree already maintains instead of
comparing `dataset.path` across every node — the same reasoning the existing
comment at `markActiveInTree` gives for a 5000-file repo. It returns `-1` both
when nothing is open and when the open file is not in the tree at all (a
gitignored target reached by a link), and stepping from outside the list puts
`next` on the first entry and `prev` on the last.

## Files touched

- `src-tauri/src/config.rs` — `Keymap::next_file` / `prev_file` + defaults.
- `ui/app.js` — fallback keymap literal; `stepFile()` at the end of the file-tree
  section; two dispatch lines in `wireKeys` after `jump_bottom`; two
  `KEY_ACTIONS` entries.
- `perf/harness/lib/fixtures.mjs`, `perf/harness/ui-check.mjs` — the one keymap
  mirror in `fixtures.mjs` and both in `ui-check.mjs`; the "every action gets a
  row" assertion bumped 15 → 17.
- `README.md` — the `[keymap]` config block.

**`src-tauri/src/main.rs` was not touched**, so nothing here is exposed to the
gap where the bin target cannot be compile-checked in this container. No new
command was needed: `default_keymap` serializes `Keymap::default()` wholesale, so
the two new fields flow through the existing command untouched.

## Verification

- The overnight build gate printed **GATE PASS** — `cargo build --lib` clean, and
  the bin still fails with exactly the 5 pre-existing macOS-gating errors and no
  new ones.
- `cargo run --example config_check` — 34 passed, 0 failed. This is the check
  that matters for a `Keymap` change: it exercises the layered-TOML merge and the
  write-back over the widened struct.
- `node --check` clean on `ui/app.js`, `perf/harness/ui-check.mjs` and
  `perf/harness/lib/fixtures.mjs`.
- `perf/harness/ui-check.mjs` could **not** be run here (the Playwright Chromium
  download is proxy-blocked). The `rows === 17` bump and the three keymap mirrors
  were updated by hand and reviewed by eye.

perf not run - pending manual check on the author's machine

## Left open

- Unverified by machine: that `]` / `[` round-trip through the settings panel's
  rebind recorder. Correct by inspection — `comboFromEvent` pushes `e.key`
  verbatim giving `"]"`, `matchCombo` lowercases both sides and finds no `+` to
  split on — but only inspection.
- On a keyboard layout where the brackets sit behind AltGr, `e.key` is still
  `"]"` but `e.altKey` may be set, and `matchCombo` requires an exact modifier
  match — so the default would not fire and would need a rebind. Not fixable
  without a layout-aware matcher; noted rather than solved.
- `stepFile` steps through *files*, never directories. If a future change makes
  directory nodes selectable, `.tree-item.file` is the selector to revisit.
