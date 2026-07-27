# Plan: two-key chord bindings (`gg`, and the leader keys after it)

Companion to the shipped half of `ideas/jump-top-bottom-keybind.md`. The jump
itself landed as `jump_top` / `jump_bottom`, defaulting to `Home` / `End`. This
document covers the part that was *not* built: the plumbing that would let a
binding be a sequence of keystrokes rather than one combo, so `gg` (and later
`m{letter}`) become expressible.

Written to be read by whoever picks this up next — including the agent on
`ideas/vim-marks-bookmark-jump.md`, whose `m{letter}` binding needs the same
machinery in a slightly different shape. See "Two shapes, one prefix" below
before assuming this plan covers that case.

## Where things stand today

`matchCombo` (`ui/app.js`) is completely stateless:

```js
function matchCombo(e, combo) {
  if (!combo) return false;
  const parts = combo.toLowerCase().split("+");
  const key = parts.pop();
  if (e.ctrlKey !== parts.includes("ctrl")) return false;
  // ...shift / alt / meta, all exact-match...
  return (e.key || "").toLowerCase() === key;
}
```

One event in, one boolean out. Nothing anywhere in the frontend remembers the
previous keystroke. The global handler in `wireKeys` is a flat if-chain of
`matchCombo` calls, each with its own `return`. The rebind UI (`startRecording`
→ `onRecordKey` → `comboFromEvent`) captures exactly one `keydown` and encodes
it as `Ctrl+Shift+X`; `comboClashes` compares whole strings; `displayCombo`
splits on `+` and swaps in `⌃⇧⌥⌘` on macOS.

So: **no chord support exists, and nothing in the current tree is a partial step
toward it.** Anyone building it starts from zero.

## Why it wasn't built with the jump

The jump itself is four lines against `scrollEl`. The chord is the whole cost of
the idea, and it is not confined to one function — it reaches `matchCombo` (8
call sites, including two inside overlay-local handlers), the recording UI, the
clash check, the combo renderer, and the `Keymap` round-trip through
`config.toml`. That is a different risk class from "add a field and an if-line",
which is what the other three keybind ideas in this batch were. `Home`/`End`
deliver the actual user-facing behaviour — jump to the ends of the document —
at that cheaper risk level, and they are rebindable, so a user who wants
`Shift+G` for the bottom already has it without any of this.

The remaining gap is narrow and worth naming honestly: **`Shift+G` is available
as a rebind today, `gg` is not.** That is the only thing this plan buys for the
jump feature. Its real value is as the foundation for marks.

## Design

### Representation

Extend the combo grammar with a space as the sequence separator, so a binding is
one-or-more space-separated combos:

```
"Ctrl+F"      one combo, exactly as today
"g g"         press g, then g
"Shift+G"     one combo, exactly as today
```

Space is chosen because it cannot appear inside a single combo (`comboFromEvent`
pushes `e.key`, which for the spacebar is `" "` — see "Traps" below, this needs
handling) and because it reads correctly in `config.toml` and in the settings
panel without a decoder ring.

This keeps every existing binding string valid and unchanged, which matters:
`config` merges raw `toml::Table`s, so a user's existing `config.toml` must keep
deserializing into the same values. It does.

### Matching

Add module-level pending state and a `matchBinding` that wraps `matchCombo`:

```js
// The one keystroke a sequence binding is waiting on. Cleared on timeout, on a
// non-matching key, and whenever an overlay opens.
let chordPending = null;   // { combo, at }
const CHORD_TIMEOUT_MS = 900;

function matchBinding(e, binding) {
  if (!binding) return false;
  const steps = binding.split(" ");
  if (steps.length === 1) return matchCombo(e, binding);   // fast path, unchanged
  // ...
}
```

Rules, in the order they must be applied:

1. If `chordPending` exists and is older than `CHORD_TIMEOUT_MS`, drop it before
   doing anything else. A stale prefix that fires an hour later is the classic
   chord bug.
2. If `chordPending` matches step 1 of this binding and the event matches step 2,
   the binding fires — and `chordPending` must be cleared *before* the action
   runs, so an action that throws does not leave the prefix armed.
3. Otherwise, if the event matches step 1 of *any* sequence binding in the
   keymap, arm `chordPending` and swallow the event (`preventDefault`, `return`
   from the handler) so the prefix key does nothing else.
4. Any other key clears `chordPending`.

Rule 3 is the one that cannot live inside `matchBinding` — deciding "is this key
a prefix for anything?" is a question about the whole keymap, not one binding.
It belongs in `wireKeys` as a single pass over the sequence bindings, computed
once per keymap load rather than per keystroke.

The pending state must also be cleared when an overlay opens, when a file is
opened, and on window blur. The cheapest correct version is a `clearChord()`
called from `openPalette` / `openSettings` / `openAnnot` / `openFile` and a
`window.addEventListener("blur", clearChord)`.

### Dispatch order

`wireKeys`'s if-chain becomes: overlay guard → `isEditable` guard → chord
resolution (rules 1–2, which must come before single-combo matching, or a
pending `g` plus a `g` bound elsewhere would race) → the existing single-combo
chain → prefix arming (rule 3) → clear (rule 4).

Note that `isEditable` already sits above all bare-letter handling; sequence
bindings must stay below it too, or typing `gg` in the annotation box jumps the
document.

### Rebind UI

`onRecordKey` currently records one key and commits. For sequences it needs a
two-state recorder:

- First keydown: if the key is a bare letter with no modifiers, do **not**
  commit yet — show `"g …"` in the button and wait up to `CHORD_TIMEOUT_MS`.
- Second keydown inside the window: commit `"g g"`.
- Timeout, or a first key that carries a modifier: commit the single combo, as
  today.

That heuristic — "a bare letter might be a prefix, a modified combo never is" —
is what keeps the existing single-combo flow feeling identical. `Ctrl+F` still
commits instantly.

`displayCombo` needs a `binding.split(" ").map(displayCombo).join(" ")` wrapper
so `g g` renders as `g g` rather than being fed to the `+`-splitter.
`comboClashes` needs no change (string equality still works) but *should* gain a
prefix-shadowing check: binding `g` to one action and `g g` to another makes the
first unreachable in a way the current clash warning will not catch.

### Backend

`Keymap` fields stay `String`. No validation is added in Rust: the frontend is
the only consumer of the grammar, and `config` already treats keymap values as
opaque strings it merges and writes back. Adding a parser in Rust would mean two
implementations of the grammar that must agree — the same trap `readCssVar` /
`modeSlice` already documents for theme parsing, and not worth repeating here
for a feature with one consumer.

### Defaults

Once the plumbing exists, `jump_top` could default to `"g g"` and `jump_bottom`
to `"Shift+G"` — but that is a *separate* decision from building the plumbing,
and it takes `Home`/`End` away from users who now have them. The better shape is
a `vim_jumps` boolean alongside `quick_highlight`: off by default, and when on it
adds `g g` / `Shift+G` as aliases rather than replacing the configured combos.
`quick_highlight` is the exact precedent — the project already treats bare-letter
shortcuts as opt-in.

## Two shapes, one prefix — a note for the marks agent

`gg` and `m{letter}` are **not** the same feature, and the plan above only
delivers the first:

| | `gg` | `m{letter}` |
|---|---|---|
| second key | fixed, known at bind time | arbitrary, is the *argument* |
| expressible as a `Keymap` string | yes, `"g g"` | no — `"m"` plus a capture |
| rebind UI | records two keys | records one key (the leader) |

What they share is exactly the pending-prefix state machine: rules 1–4 above,
the timeout, and the clearing discipline. What they do not share is the
representation. Marks wants `set_mark: "m"` / `jump_mark: "'"` as ordinary
single-combo bindings, plus a `captureNext` mode that grabs the following
keystroke as data and hands it to the action.

If both get built, build `captureNext` as the primitive and express `gg` on top
of it (`g` arms, second key is checked against the fixed expectation) rather
than the other way round — the arbitrary-key case is strictly more general, and
a fixed-sequence matcher does not extend to it.

**If marks lands first and ships `captureNext`, this plan's rules 1–4 are
already done and only the representation and rebind-UI sections remain.**

> **Status update — marks landed first.** `set_mark` / `jump_mark` shipped with
> the `captureNext` primitive (`pendingMark`, `armMark`, `clearMark`,
> `consumeMarkKey` in `ui/app.js`), so rules 1–4 above now exist in the tree and
> this plan's scope has shrunk to **representation + rebind UI only**. What was
> built differs from rules 1–4 in one deliberate way worth knowing before you
> build on it: the marks machine never swallows an unrecognised key. A modified
> combo or a non-alphanumeric cancels the pending leader and *falls through* to
> the normal chain, so the blast radius is one keystroke at most. A fixed
> sequence like `g g` needs the opposite for its second step — a `g` followed by
> a non-`g` should arguably still be swallowed — so budget for a per-kind policy
> rather than reusing `consumeMarkKey` verbatim. Rule 3 (arming from a whole-
> keymap prefix scan) is still unbuilt: marks arm from an ordinary `matchCombo`
> hit in the if-chain, because their leaders *are* ordinary single combos.
> The spacebar trap is still live and still untouched — nothing in marks needed
> the space-separated grammar.

## Traps

- **The spacebar.** `comboFromEvent` pushes `e.key`, which is `" "` for space,
  so a space-separated grammar makes a spacebar binding ambiguous. Encode it as
  `"Space"` in `comboFromEvent` and decode it in `matchCombo` before this ships.
  Nothing binds space today, so this is a latent bug the grammar would activate,
  not one it inherits.
- **Key repeat.** Holding `g` fires repeated `keydown`s with `e.repeat === true`.
  A chord must ignore repeats or holding the prefix key self-triggers.
- **Dead keys and IME.** `e.key` can be `"Dead"` or `"Process"`. Treat both as
  "clears the prefix", never as a step.
- **`Home`/`End` inside the palette input.** Already handled — the overlay guard
  in `wireKeys` returns before any binding is consulted — but a chord arming
  *before* the overlay opens must be cleared, hence `clearChord()` in the open
  paths.

## Cost

Roughly 80–110 lines in `ui/app.js` across `matchCombo`/`matchBinding`,
`wireKeys`, `onRecordKey`, `comboFromEvent`, `displayCombo`, `comboClashes`, and
the clear-on-open paths. No Rust beyond a default-value change if `vim_jumps`
lands. `perf/harness/ui-check.mjs` would want a chord case in the keys tab, and
it cannot run in CI today — this is a change to verify by hand in the real app.
