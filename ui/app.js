"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ---- appearance ----------------------------------------------------------
// Set before anything paints. `mode = "system"` is the default, and matching the
// OS is the whole of what we can know without an IPC — `loadTheme()` corrects
// this a few round trips later if the config pins an appearance, and until then
// the native window background (which Rust already painted from the resolved
// mode) is what is actually on screen.
//
// This lives here rather than in an inline <script> in index.html because the
// CSP is `script-src 'self'` with no 'unsafe-inline': an inline script is
// blocked, silently. `<script src="app.js" defer>` runs after parsing and
// before DOMContentLoaded, which is early enough.
const prefersDark = window.matchMedia("(prefers-color-scheme: dark)");
let appearance = prefersDark.matches ? "dark" : "light";
document.documentElement.dataset.mode = appearance;
// The user's preference, as opposed to what it resolved to. Only "system"
// makes the OS listeners live.
let modePref = "system";

// ---- perf instrumentation ------------------------------------------------
// Marks are forwarded to Rust and printed as NDJSON on stderr — console.log
// inside WKWebView never reaches the process's stdout, so this is the only way
// to get real webview timings out of a running app. See src-tauri/src/perf.rs.
//
// `perf.on` is false unless the binary was built with `--features perf`, and
// every method short-circuits when it is, so a normal build does no work here
// beyond the single `perf_enabled` probe at startup. That probe disappears
// once the startup IPCs are collapsed into one call.
//
// Phase naming: `d:` prefixed phases are *durations* in ms; everything else is
// a timestamp on the webview's `performance.now()` clock.
const perf = {
  on: false,
  async probe() {
    try {
      perf.on = await invoke("perf_enabled");
    } catch {
      perf.on = false;
    }
  },
  /** Timestamp mark. */
  at(phase) {
    if (perf.on) invoke("perf_mark", { phase, ms: performance.now() }).catch(() => {});
  },
  /** Duration mark, measured from `t0` (a prior `performance.now()`). */
  span(phase, t0) {
    if (perf.on) {
      invoke("perf_mark", { phase: "d:" + phase, ms: performance.now() - t0 }).catch(() => {});
    }
  },
  now: () => performance.now(),
};

// ---- app state -----------------------------------------------------------
let currentFile = null;
let repoRoot = "";
// False until something has been opened. Only ever false on a launch with no
// path and no `.git` above the cwd — a .app double-clicked from Finder, where
// the window is hidden and the menubar is the whole UI until File → Open.
let hasRepo = true;
// Overwritten wholesale from Rust at startup, and again whenever the settings
// panel saves — rebinding a key takes effect without a restart.
// Every field `Keymap` has, because this literal is what the app runs on until
// the `get_keymap` round trip lands — a key missing here is one `matchCombo`
// asks about as `undefined` and answers "no" to, i.e. an action that is quietly
// dead for the first few frames.
let keymap = {
  // Not a binding: how the `Ctrl+` in the combos below is spelled on the
  // keyboard. "linux" | "mac" | "vim" — see `resolveCombo`.
  mode: "linux",
  palette: "Ctrl+F",
  palette_prev: "Ctrl+P",
  palette_next: "Ctrl+N",
  highlight: "Ctrl+Shift+H",
  send_stack: "Ctrl+Enter",
  send_stack_tmux: null,
  toggle_stack: "Ctrl+O",
  copy_stack: "Ctrl+C",
  settings: "Ctrl+,",
  save_annotation: "Ctrl+Y",
  toggle_outline: "Ctrl+I",
  toggle_pane: "Ctrl+T",
  toggle_tree: "Ctrl+B",
  toggle_view: "Ctrl+M",
  jump_top: "Home",
  jump_bottom: "End",
  scroll_down: "j",
  scroll_up: "k",
  scroll_half_down: "d",
  scroll_half_up: "u",
  pane_left: "Ctrl+H",
  pane_right: "Ctrl+J",
  next_file: "]",
  prev_file: "[",
  set_mark: "m",
  jump_mark: "'",
  jump_back: "Ctrl+[",
  jump_forward: "Ctrl+]",
  find: "/",
  find_next: "n",
  find_prev: "Shift+N",
  quick_highlight: true,
};
let pending = null; // { id, mark } while awaiting an annotation
let highlightMode = false; // highlighter tool: auto-highlight on selection

const $ = (id) => document.getElementById(id);
const contentEl = $("content");
const scrollEl = $("content-scroll");
const staleRail = $("stale-rail");

// ---- init ----------------------------------------------------------------
async function init() {
  await perf.probe();
  perf.at("js_start");
  if (/Macintosh/.test(navigator.userAgent)) document.body.classList.add("mac");

  // First, because it decides whether the sidebar stays collapsed. The markup
  // ships collapsed and we *remove* the class here, so the single-file case is
  // deterministically flash-free; a directory launch gets its sidebar a few ms
  // into JS boot, which is invisible against the time the window took to exist.
  const initial = await invoke("initial_file").catch(() => null);
  if (!initial) document.body.classList.remove("nav-collapsed");

  // Theme next: index.html only carries fallback colours, so every IPC we do
  // ahead of this is time the window spends in the default theme rather than
  // the user's.
  await loadTheme();
  perf.at("ipc_theme");

  try {
    await adoptRepoInfo();
  } catch (e) { console.error(e); }
  perf.at("ipc_repo_info");

  // One round trip, two answers: the tree, the stack panel and the agent pane
  // cannot lay themselves out at their persisted sizes without `[ui]`, and
  // asking for it after the keymap would put a second serial IPC in front of
  // the first paint. `get_ui` is a lock read — `get_settings`, which also
  // carries it, walks the themes directory.
  try {
    const [km, ui] = await Promise.all([invoke("get_keymap"), invoke("get_ui")]);
    if (km) keymap = km;
    applyPanelSizes(ui);
  } catch (e) {}
  // `displayCombo`, not the raw value: what is stored is `Ctrl+F` in every key
  // mode, and telling a reader in `vim` mode to press Ctrl+F is telling them
  // to press a combo that does nothing.
  $("search-hint").textContent = `Press ${displayCombo(keymap.palette)} to search`;
  perf.at("ipc_keymap");

  wireEvents();
  wireKeys();
  wireUi();
  wireTooltips();
  perf.at("wired");

  // The tree is off the critical path when a document is opening: Rust blocks
  // until the background walk lands, so this simply resolves late and
  // `paintTree` marks the open file active whenever it does. `.catch` is
  // load-bearing — left unawaited, a rejection here is an unhandled one.
  const tree = loadTree().catch((e) => console.error(e));

  // Same treatment, and for the same reason: the badge is chrome, not the
  // document. Unawaited so `first_paint` below measures the document arriving
  // rather than a `get_stack` round trip — the store is empty at boot today,
  // but it stops being empty the moment an agent or a loaded file populates it,
  // and a boot that never asked would paint a badge of nothing over a full
  // stack. `.catch` is load-bearing for the same reason it is on `loadTree`.
  refreshStack().catch((e) => console.error(e));

  // nvim-style: `dreamd file.md` opens the file on load.
  if (initial) await openFile(initial).catch((e) => console.error(e));
  else await tree;
  perf.at("first_paint");
}

// The syntect theme baked into the last render. Code-block colours are inline
// styles produced in Rust, not CSS, so they only change on a re-render — and a
// re-render is far too expensive to do on every save of a theme being edited.
let appliedSyntaxTheme = null;

async function loadTheme() {
  try {
    await applyTheme(await invoke("get_theme"));
  } catch (e) { console.error(e); }
}

/// Adopt a `{css, mode, scheme}` view from Rust: appearance first, then the
/// stylesheet, so there is no frame in between showing one against the other.
async function applyTheme(view) {
  modePref = view.mode;
  appearance = view.scheme;
  document.documentElement.dataset.mode = appearance;
  $("user-theme").textContent = view.css;
  // Read the *applied* value rather than re-parsing the text: the engine has
  // already done the cascade, the specificity, the quotes and any @media, and a
  // value it resolved cannot drift from `theme::custom_property` the way a
  // second implementation would.
  // The terminal paints its own colours from a JS object, so a palette or
  // appearance change has to be pushed into it — the CSS swap above cannot
  // reach inside xterm's canvas. No-op until the pane has been opened once.
  if (pty.term) pty.term.options.theme = terminalTheme();
  const syntax = appliedCssVar("--syntax-theme");
  const changed = appliedSyntaxTheme !== null && syntax !== appliedSyntaxTheme;
  appliedSyntaxTheme = syntax;
  // Code colours are inline styles baked in by syntect, so they cannot follow a
  // CSS attribute swap — only a re-render moves them. Nothing on disk changed,
  // so this must not drag re-anchoring along with it.
  if (changed) await renderCurrent({ preserveScroll: true, reanchor: false });
}

/// Follow the OS appearance. Inert unless the preference is "system", and
/// guarded there rather than at registration time so flipping the setting back
/// to "system" starts working without re-wiring anything.
async function applyAppearance(next) {
  if (modePref !== "system" || next === appearance) return;
  try {
    // Awaited, and it returns the new view: `render_markdown` reads the scheme
    // on the Rust side to pick the syntect theme, so a re-render that raced
    // this would bake the old code colours into the new palette.
    await applyTheme(await invoke("set_appearance", { scheme: next }));
  } catch (e) { console.error(e); }
}

/// A custom property as the engine resolved it on the live document. Quotes
/// stripped, so `--syntax-theme: "Solarized (light)"` comes back bare.
function appliedCssVar(name) {
  const raw = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return raw.replace(/^["']|["']$/g, "") || null;
}

/// Drop `/* … */` blocks. Every reader below has to do this first: the base
/// stylesheet's own header comment contains a `:root { --bg: … }` example, and
/// a matcher that doesn't strip comments happily parses the documentation
/// instead of the theme. Mirrors `theme::strip_comments` on the Rust side.
function stripCssComments(css) {
  return String(css).replace(/\/\*[\s\S]*?(?:\*\/|$)/g, "");
}

/// The last declaration of a custom property *in `mode`*, quotes stripped.
/// Mirrors `theme::custom_property`.
///
/// Only for stylesheets that are *not* applied to the document — the theme-card
/// swatches and the Custom tab. For the live theme use `appliedCssVar`, which
/// asks the engine instead of re-implementing it.
function readCssVar(css, name, mode) {
  const scoped = modeSlice(stripCssComments(css), mode);
  const m = scoped.match(new RegExp(`(?:^|[^\\w-])${name}\\s*:([^;}]*)`, "g"));
  if (!m || !m.length) return null;
  const last = m[m.length - 1];
  return last.slice(last.indexOf(":") + 1).trim().replace(/^["']|["']$/g, "");
}

/// The stylesheet as it applies in `mode`: the other appearance's blocks
/// dropped, this one's moved to the end. Mirrors `theme::mode_slice`, including
/// the rule that carries backwards compatibility — a stylesheet with no
/// `[data-mode]` block is returned untouched, so every palette written before
/// families, and every hand-written `theme_css`, reads exactly as it did.
///
/// Moving rather than merely dropping matters for the same reason it does in
/// Rust: this is a last-wins textual scan, but real CSS ranks
/// `:root[data-mode="dark"]` above `:root` whatever the source order.
function modeSlice(css, mode) {
  if (!mode || !css.includes("data-mode")) return css;
  let kept = "", mine = "", copied = 0, prelude = 0, i = 0;
  while (i < css.length) {
    const c = css[i];
    if (c === '"' || c === "'") { i = skipString(css, i); continue; }
    if (c === "{") {
      const named = modeAttr(css.slice(prelude, i));
      if (named === undefined) { prelude = ++i; continue; }  // descend
      const end = blockEnd(css, i);
      kept += css.slice(copied, prelude);
      if (named === mode) mine += css.slice(prelude, end);
      i = copied = prelude = end;
      continue;
    }
    if (c === "}" || c === ";") { prelude = ++i; continue; }
    i++;
  }
  return kept + css.slice(copied) + mine;
}

/// `undefined` when a selector has no `data-mode`; `null` when it names one we
/// don't recognise, which belongs to neither appearance.
function modeAttr(prelude) {
  const m = /data-mode\s*[~|^$*]?=\s*("([^"]*)"|'([^']*)'|[^\]\s]+)/i.exec(prelude);
  if (!m) return undefined;
  const value = (m[2] ?? m[3] ?? m[1]).trim().toLowerCase();
  return value === "dark" || value === "light" ? value : null;
}

function blockEnd(css, open) {
  let depth = 0;
  for (let i = open; i < css.length; i++) {
    const c = css[i];
    if (c === '"' || c === "'") { i = skipString(css, i) - 1; continue; }
    if (c === "{") depth++;
    else if (c === "}" && --depth === 0) return i + 1;
  }
  return css.length;
}

function skipString(css, start) {
  const quote = css[start];
  for (let i = start + 1; i < css.length; i++) {
    if (css[i] === "\\") { i++; continue; }
    if (css[i] === quote) return i + 1;
  }
  return css.length;
}

// ---- file tree -----------------------------------------------------------
// `ipc_tree` is marked here rather than on the boot path because the tree no
// longer sits on it: on a single-file launch this resolves after `first_paint`.
// The mark has changed meaning — it is "when the sidebar painted", not a
// cumulative boot timestamp — so it is not comparable to the old baseline.
async function loadTree() {
  paintTree(await invoke("list_markdown_files"));
  perf.at("ipc_tree");
}

// The header's idea of which repo is open. Split out of `init` because
// File → Open moves the root at runtime and the same three fields have to
// follow it.
async function adoptRepoInfo() {
  const info = await invoke("repo_info");
  repoRoot = info.root || "";
  hasRepo = info.hasRepo !== false;
  paintRootField();
}

// ---- root path field -----------------------------------------------------
// `#repo-name` is the tree's heading and the way to move the tree's root, in
// one element (D22). Basename when it is not focused, because that is a
// heading; the full path when it is, because that is what you edit. `~` is
// expanded and the completion list built in Rust — see `rootfield.rs` for what
// that surface is allowed to see.
//
// There is no history and no dropdown: the field remembers nothing across a
// session and writes nothing to `~/.config/dreamd/`.

function basenameOf(path) {
  const trimmed = String(path || "").replace(/\/+$/, "");
  return trimmed.slice(trimmed.lastIndexOf("/") + 1) || trimmed;
}

// The one place the field's text comes from. Called on boot, after every
// `repo-changed`, and on blur — so an abandoned edit is discarded rather than
// left looking like the root.
function paintRootField() {
  const el = $("repo-name");
  el.classList.remove("error");
  el.title = hasRepo ? repoRoot : "";
  if (el === document.activeElement) el.value = repoRoot;
  else el.value = hasRepo ? basenameOf(repoRoot) || repoRoot : "no repo";
}

function wireRootField() {
  const el = $("repo-name");
  el.addEventListener("focus", () => {
    el.value = hasRepo ? repoRoot : "";
    // Selected, so the first keystroke replaces a path nobody wants to erase
    // by hand. Synchronously, and then the mouseup that is about to arrive is
    // swallowed: a click focuses *first* and places the caret *after*, which
    // would otherwise collapse the selection made here. Deferring the select
    // to that mouseup instead (or to a frame later) races anything already
    // typing into the field.
    el.select();
    el.addEventListener("mouseup", (e) => e.preventDefault(), { once: true });
  });
  el.addEventListener("blur", paintRootField);
  el.addEventListener("input", () => el.classList.remove("error"));
  el.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      // Not the document's Escape: that one closes overlays and leaves view
      // mode, and abandoning an edit should do neither.
      e.preventDefault();
      e.stopPropagation();
      el.blur();
    } else if (e.key === "Tab" && !e.shiftKey) {
      e.preventDefault();
      completeRoot(el);
    } else if (e.key === "Enter") {
      e.preventDefault();
      submitRoot(el);
    }
  });
}

async function submitRoot(el) {
  const text = el.value.trim();
  if (!text || text === repoRoot) { el.blur(); return; }
  try {
    await invoke("set_root", { path: text });
    // Nothing else to do here. `adopt_root` swaps config, re-walks, re-arms
    // the watcher, flushes marks, retires and re-binds the MCP socket, and
    // then emits `repo-changed` — whose handler is the one that repaints.
    el.blur();
  } catch (err) {
    // The error state leaves you in the current root: the text you typed
    // stays so it can be corrected, and nothing behind it moved.
    el.classList.add("error");
    toast(String(err));
  }
}

// Tab completion, directories only. One match completes; several extend as far
// as they agree, which is the most that can be typed for you without choosing
// between them — and the names are toasted rather than given a dropdown.
async function completeRoot(el) {
  const text = el.value;
  let names;
  try {
    names = await invoke("complete_directories", { path: text });
  } catch (err) {
    el.classList.add("error");
    toast(String(err));
    return;
  }
  if (!names || !names.length) { toast("Nothing completes that"); return; }
  const cut = text.lastIndexOf("/") + 1;
  const typed = text.slice(cut);
  const shared = names.reduce(commonPrefix);
  // `shared` can be shorter than what was typed — the match is case-insensitive
  // and `Notes` and `nothing` agree on nothing — and completing must never
  // delete characters.
  const insert = names.length === 1
    ? names[0] + "/"
    : shared.length > typed.length ? shared : typed;
  el.value = text.slice(0, cut) + insert;
  el.setSelectionRange(el.value.length, el.value.length);
  if (names.length > 1) {
    toast(names.slice(0, 8).join("   ") + (names.length > 8 ? `   …+${names.length - 8}` : ""));
  }
}

function commonPrefix(a, b) {
  let i = 0;
  while (i < a.length && i < b.length && a[i] === b[i]) i++;
  return a.slice(0, i);
}

// ---- panel sizes ---------------------------------------------------------
// Four numbers and one gesture. The handle on the sidebar's right border, the
// one on the stack panel's left, and the one on whichever edge the agent pane
// is docked to all do the same three things: clamp, write a CSS variable, and
// debounce a `set_config`. They are one mechanism rather than three copies
// because the only thing that actually differs between them is which fixed
// edge the pointer is being measured from.
//
// Rust clamps every one of these on the way in, so a stale or hand-typed value
// costs the nearest usable panel rather than a rejected config file; the ranges
// here mirror `config::TREE_WIDTH_*` / `STACK_WIDTH_*` / `PANE_WIDTH_*` /
// `PANE_HEIGHT_*` and exist so a drag never *sends* something out of range in
// the first place. The fallbacks in index.html mirror the defaults.
const SIZES = {
  tree:  { key: "tree_width",  css: "--tree-width",  min: 140, max: 600,  def: 260 },
  stack: { key: "stack_width", css: "--stack-width", min: 200, max: 720,  def: 280 },
  paneW: { key: "pane_width",  css: "--pane-width",  min: 240, max: 1200, def: 380 },
  paneH: { key: "pane_height", css: "--pane-height", min: 120, max: 1200, def: 240 },
};

// The last value applied for each, which is what gets persisted — never the
// raw pointer position, so the file can only ever hold something in range.
const sized = {};

// Written as an inline style on <html>, which outranks any `:root` rule a
// palette might carry.
function applySize(name, px) {
  const s = SIZES[name];
  const want = Number(px) || s.def;
  sized[name] = Math.round(Math.min(s.max, Math.max(s.min, want)));
  document.documentElement.style.setProperty(s.css, `${sized[name]}px`);
}

// Every persisted size at once, from the one `get_ui` the boot already does.
// `undefined` is fine at each of them — `applySize` falls back to the default,
// which is the same number index.html's `var()` fallback already painted, so a
// config that has never mentioned `[ui]` costs no reflow.
function applyPanelSizes(ui) {
  applySize("tree", ui && ui.tree_width);
  applySize("stack", ui && ui.stack_width);
  applySize("paneW", ui && ui.pane_width);
  applySize("paneH", ui && ui.pane_height);
}

// Debounced: a drag across the window is one config write, not forty. One
// timer and one accumulating patch across all four, so resizing two panels in
// quick succession is still a single write rather than two racing ones — the
// global table is patched and renamed over, and the loser of that race would
// take the winner's key with it.
let sizeTimer = null;
let sizePatch = {};
function persistSize(name) {
  sizePatch[SIZES[name].key] = sized[name];
  clearTimeout(sizeTimer);
  sizeTimer = setTimeout(() => {
    const ui = sizePatch;
    sizePatch = {};
    invoke("set_config", { patch: { ui } }).catch((e) => console.error(e));
  }, 400);
}

// `begin` is handed the pointerdown and answers what *this* press is dragging:
// the size it writes, the class that holds the cursor for its duration, and the
// function turning each subsequent move into a number. Resolved per press
// rather than per handle for two reasons — the pane has to pick its axis at
// press time, because `agent.position` is a live setting and the panel can
// change it under a wired handler; and every measurement is taken from a fixed
// edge that has to be read *before* the box starts moving underneath the drag.
//
// `atMinimum` is the tree's alone: only it has somewhere to go past its own
// minimum. Returning true from it means "this move sets no size".
function wireDrag(handleId, begin) {
  $(handleId).addEventListener("pointerdown", (e) => {
    // No `setPointerCapture`: dragging the tree past its minimum collapses it,
    // which hides that handle, and a capture held by a `display: none` element
    // is not something to rely on. Window listeners for the length of the drag
    // do the same job and cannot be lost that way.
    e.preventDefault();
    const { name, cls, sizeOf, atMinimum } = begin(e);
    document.body.classList.add(cls);
    const onMove = (ev) => {
      const raw = sizeOf(ev);
      if (atMinimum && atMinimum(raw)) return;
      applySize(name, raw);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      document.body.classList.remove(cls);
      persistSize(name);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  });
}

function wireResizeHandles() {
  // The tree grows rightwards from the window's left edge.
  wireDrag("tree-resize", () => {
    const left = $("sidebar").getBoundingClientRect().left;
    return {
      name: "tree", cls: "dragging-tree",
      sizeOf: (ev) => ev.clientX - left,
      atMinimum: (raw) => {
        // D20: at the extreme the drag *is* `toggle_tree`. The width is left
        // at the last usable one, so expanding again restores what you had
        // rather than snapping back to the default.
        if (raw >= SIZES.tree.min) { document.body.classList.remove("nav-collapsed"); return false; }
        document.body.classList.add("nav-collapsed");
        return true;
      },
    };
  });

  // The stack panel grows leftwards from its own right edge — the window's
  // right edge, or the agent pane's left one when that is docked right.
  wireDrag("stack-resize", () => {
    const right = $("stack-panel").getBoundingClientRect().right;
    return { name: "stack", cls: "dragging-stack", sizeOf: (ev) => right - ev.clientX };
  });

  // The pane grows from whichever edge it is *not* docked against, so the dock
  // decides both the axis and which of the two keys the drag writes.
  //
  // No collapse at the minimum here, deliberately: a pane dragged shut would
  // read as having taken the running child with it while the process carried
  // on, and `toggle_pane` — which hides it and says the process is still there
  // — is already the gesture that means this.
  wireDrag("pane-resize", () => {
    const right = document.body.classList.contains("agent-right");
    const box = $("pty-pane").getBoundingClientRect();
    return {
      name: right ? "paneW" : "paneH", cls: "dragging-pane",
      sizeOf: right ? (ev) => box.right - ev.clientX : (ev) => box.bottom - ev.clientY,
    };
  });
}

// Every file path the tree currently shows. The watcher cannot tell a rename
// that creates a file from a rename that replaces one, so it emits `file-added`
// for both; this is what lets the ordinary atomic-replace save be dropped
// without a repo walk. Rebuilt from the tree itself, so it can never disagree
// with what is on screen.
const knownPaths = new Set();

function collectPaths(node, into) {
  if (node.is_dir) for (const c of node.children) collectPaths(c, into);
  else into.add(node.path);
}

// Split out so callers that already hold a fresh tree — `rebuild_index`
// returns one — don't have to ask Rust to walk the repo again for it.
function paintTree(root) {
  const tree = $("tree");
  tree.innerHTML = "";
  knownPaths.clear();
  collectPaths(root, knownPaths);
  // Render the root's children directly (skip the root dir node itself).
  for (const child of root.children) tree.appendChild(renderNode(child));
  // An empty sidebar used to render as literally nothing, which reads as a bug
  // rather than an answer. Two ways to get here: a repo with no markdown in it,
  // and a launch with no repo at all (a .app double-clicked from Finder), where
  // the menubar is the only way forward — so say which one it is.
  if (!root.children.length) {
    const empty = document.createElement("div");
    empty.className = "tree-empty";
    empty.textContent = hasRepo
      ? "No markdown files here."
      : "Nothing open yet — File ▸ Open Folder…";
    tree.appendChild(empty);
  }
  activeTreeItem = null;
  markActiveInTree(currentFile);
}

function renderNode(node) {
  if (node.is_dir) {
    const wrap = document.createElement("div");
    wrap.className = "tree-dir";
    const label = document.createElement("div");
    label.className = "tree-item tree-label";
    label.textContent = "▾ " + node.name;
    const children = document.createElement("div");
    children.className = "tree-children";
    for (const c of node.children) children.appendChild(renderNode(c));
    let open = true;
    label.onclick = () => {
      open = !open;
      children.style.display = open ? "" : "none";
      label.textContent = (open ? "▾ " : "▸ ") + node.name;
    };
    wrap.appendChild(label);
    wrap.appendChild(children);
    return wrap;
  }
  const item = document.createElement("div");
  item.className = "tree-item file";
  item.dataset.path = node.path;
  const name = document.createElement("span");
  name.className = "file-name";
  name.textContent = node.name;
  name.onclick = () => openFile(node.path);
  const opts = document.createElement("button");
  opts.className = "file-opts";
  opts.textContent = "⋯";
  opts.dataset.tip = "File options";
  opts.onclick = (e) => { e.stopPropagation(); openFileMenu(opts, node); };
  item.appendChild(name);
  item.appendChild(opts);
  return item;
}

// The active file is tracked by reference rather than found by querying every
// file node: on a 5000-file repo the query-and-toggle form did 5000 class
// writes — 5000 style invalidations — on every single file open.
let activeTreeItem = null;

function markActiveInTree(path) {
  if (activeTreeItem) activeTreeItem.classList.remove("active");
  activeTreeItem = path
    ? $("tree").querySelector(`.tree-item.file[data-path="${cssEscape(path)}"]`)
    : null;
  if (activeTreeItem) activeTreeItem.classList.add("active");
}

function cssEscape(s) {
  return window.CSS && CSS.escape ? CSS.escape(s) : s.replace(/["\\]/g, "\\$&");
}

// Open the next / previous file in the sidebar's own order.
//
// The sidebar DOM *is* the flattened tree: `renderNode` emits `.tree-item.file`
// depth-first, so document order here is already exactly the order on screen.
// Reading it back beats re-deriving a flat list from `FileNode` in Rust — there
// is only ever one ordering, and it follows every repaint (`rebuild_index`, the
// watcher's add/remove, File ▸ Open moving the root) with no second thing to
// keep in step and no new IPC round trip per keystroke.
//
// Collapsed directories are deliberately **not** skipped. Collapsing sets
// `display:none` on the children and leaves them in the DOM, and the whole point
// of this binding is to move without touching the tree — which may be collapsed
// entirely (`nav-collapsed`) or hidden outright by view mode. Skipping would make
// files unreachable by keyboard depending on state the user may not be looking at.
//
// Wraps at both ends, matching `movePalette`. There is no affordance for "that
// was the last file", so stopping would be a silent no-op indistinguishable from
// a dead key.
function stepFile(d) {
  const files = $("tree").querySelectorAll(".tree-item.file");
  if (!files.length) return;
  // `activeTreeItem` is one of these nodes by construction (`markActiveInTree`
  // queries the same list, `paintTree` re-resolves it), so this avoids a scan
  // over `dataset.path` on every keypress. -1 when it is null, and also when the
  // open file isn't in the tree at all — a gitignored target reached by a link —
  // in which case stepping from just outside the list puts `next` on the first
  // entry and `prev` on the last.
  const i = [].indexOf.call(files, activeTreeItem);
  const el = files[i < 0 ? (d > 0 ? 0 : files.length - 1)
                        : (i + d + files.length) % files.length];
  // The sidebar scrolls independently of the reading pane; without this the
  // active row walks off the top of it after a few presses.
  el.scrollIntoView({ block: "nearest" });
  openFile(el.dataset.path);
}

// ---- open / render -------------------------------------------------------
async function openFile(path) {
  // The one cross-file entry point, so the one place the jump history needs to
  // record from. Guarded three ways: `here()` is null at boot when there is
  // nothing to come back to, `restoring` marks an arrival rather than a
  // departure, and re-opening the file already open is not a move.
  if (!restoring && path !== currentFile) pushJump(here());
  currentFile = path;
  markActiveInTree(path);
  await renderCurrent({ preserveScroll: false, reanchor: false });
}

/// Re-render the open document.
///
/// The two flags used to be one. `preserveScroll` also decided whether to
/// re-anchor, which is right for a file that changed on disk and wrong for a
/// theme or appearance switch: nothing moved, so re-anchoring every highlight
/// is work for no result — and on a large document during an OS auto-switch,
/// a visible one.
async function renderCurrent({ preserveScroll, reanchor }) {
  if (!currentFile) return;
  const t0 = perf.now();
  const prevScroll = preserveScroll ? scrollEl.scrollTop : 0;
  // Before the `innerHTML` write below, not after: every stored find `Range`
  // points into the DOM about to be replaced.
  invalidateFind();
  let html;
  try {
    html = await invoke("render_markdown", { path: currentFile });
  } catch (e) {
    contentEl.innerHTML = `<div class="empty">${escapeHtml(String(e))}</div>`;
    refreshOutline();
    if (findQuery) findRecompute(false);
    return;
  }
  perf.span("ipc_render_markdown", t0);

  let t = perf.now();
  contentEl.innerHTML = html;
  perf.span("innerhtml", t);

  t = perf.now();
  interceptLinks();
  perf.span("intercept_links", t);

  t = perf.now();
  decorateCodeBlocks();
  perf.span("decorate_code", t);

  t = perf.now();
  const highlights = reanchor
    ? await invoke("reanchor", { path: currentFile })
    : await invoke("get_highlights", { path: currentFile });
  perf.span(reanchor ? "ipc_reanchor" : "ipc_get_highlights", t);

  t = perf.now();
  applyHighlights(highlights);
  perf.span("apply_highlights", t);

  refreshOutline();

  scrollEl.scrollTop = prevScroll;
  // The search equivalent of `reanchor`, and the reason `invalidateFind` above
  // is not the whole story: without this, a `:w` in Neovim under an open find
  // bar leaves stale paint and a dead `n`. `move: false` — the reader asked for
  // a save, not a jump, so the pane stays where the line above put it.
  if (findQuery) findRecompute(false);
  perf.span("render_total", t0);
}

// External links open in the OS browser; internal .md links navigate in-app.
function interceptLinks() {
  contentEl.querySelectorAll("a[href]").forEach((a) => {
    const href = a.getAttribute("href");
    if (!href) return;
    if (/^[a-z]+:\/\//i.test(href) || href.startsWith("mailto:")) {
      a.onclick = (e) => { e.preventDefault(); invoke("open_external", { url: href }); };
    } else if (href.startsWith("#")) {
      // Frame captured before the scroll and pushed only if the target was
      // actually found — a `#anchor` naming a heading that isn't there moves
      // nobody, and recording it would put a "back" on the stack that goes
      // where you already are.
      a.onclick = (e) => {
        e.preventDefault();
        const from = here();
        if (scrollToFragment(href.slice(1))) pushJump(from);
      };
    } else {
      // relative path -> only navigate to markdown inside the repo; other
      // relative targets are dropped (never handed to the OS opener).
      a.onclick = (e) => {
        e.preventDefault();
        const base = currentFile.replace(/[^\/]*$/, "");
        // Split on the *first* `#` only: everything after it is the fragment,
        // even if it contains further `#`s.
        const cut = href.indexOf("#");
        const rel = cut === -1 ? href : href.slice(0, cut);
        const frag = cut === -1 ? "" : href.slice(cut + 1);
        const target = normalizePath(base + rel);
        if (!/\.(md|markdown|mdown|mkd)$/i.test(target)) {
          toast("Ignored non-markdown local link");
        } else if (!insideRepo(target, repoRoot)) {
          // Enough `../` segments resolve outside the root; images have always
          // refused that and links now do too.
          toast("Ignored link outside the repo");
        } else if (frag) {
          // Cross-file section link. `renderCurrent` sets scrollTop last, so
          // awaiting the open is what makes this land after the reset.
          openFile(target).then(() => scrollToFragment(frag), () => {});
        } else {
          openFile(target);
        }
      };
    }
  });
  // resolve relative image src
  contentEl.querySelectorAll("img[src]").forEach((img) => {
    const src = img.getAttribute("src");
    if (src && !/^[a-z]+:\/\//i.test(src) && !src.startsWith("data:") && !src.startsWith("/")) {
      const base = currentFile.replace(/[^\/]*$/, "");
      const abs = normalizePath(base + src);
      // Only load local images that live inside the repo root.
      if (insideRepo(abs, repoRoot)) img.src = "file://" + abs;
      else img.removeAttribute("src");
    }
  });
}

// ---- code blocks ---------------------------------------------------------

// The copy button's contents. Author-written and static — nothing from the
// document is interpolated into it, so `innerHTML` here parses only this
// string. Deliberately no text and no <svg><title>: the button must contribute
// zero text nodes to #content (see the CSS note in index.html). The two icons
// are both present and CSS picks one, so toggling state never touches the DOM
// shape.
const COPY_ICON_SVG =
  '<svg class="ic-copy" viewBox="0 0 24 24" fill="none" stroke="currentColor"' +
  ' stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<rect x="9" y="9" width="11" height="11" rx="2"></rect>' +
  '<path d="M5 15V5a2 2 0 0 1 2-2h10"></path></svg>' +
  '<svg class="ic-check" viewBox="0 0 24 24" fill="none" stroke="currentColor"' +
  ' stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<path d="M20 6 9 17l-5-5"></path></svg>';

/// Give every rendered code block a copy button, top right.
///
/// Post-render DOM decoration rather than markup from `markdown::render`: the
/// button is chrome, not document content, and keeping it out of the render
/// pass keeps it out of anything that reads the rendered HTML. It runs on the
/// fresh DOM after each render, like `interceptLinks` — `renderCurrent`'s
/// `innerHTML` assignment throws the previous buttons away, which is also what
/// makes them survive a `file-changed` re-render with nothing to clean up.
///
/// Called *before* `applyHighlights` so the DOM shape is settled before any
/// mark is placed; the button adds no text nodes either way, so neither the
/// text-node scan nor `getSelection().toString()` can see it.
function decorateCodeBlocks() {
  for (const pre of contentEl.querySelectorAll("pre")) {
    const parent = pre.parentNode;
    if (!parent) continue;
    // Idempotent: a re-run over an already-decorated block is a no-op. Nothing
    // calls it twice today, but it is one line and removes the whole class of
    // double-button bug.
    if (parent.classList && parent.classList.contains("code-block")) continue;

    const wrap = document.createElement("div");
    wrap.className = "code-block";
    parent.insertBefore(wrap, pre);
    wrap.appendChild(pre);

    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "icon code-copy";
    btn.setAttribute("aria-label", "Copy code");
    btn.dataset.tip = "Copy code";
    btn.innerHTML = COPY_ICON_SVG;
    btn.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      copyCodeBlock(pre, btn);
    });
    // #content's own `mouseup` listener starts a highlight whenever the
    // selection is non-empty, and a leftover selection elsewhere in the
    // document would otherwise make a copy click open the annotation modal.
    btn.addEventListener("mouseup", (e) => e.stopPropagation());
    wrap.appendChild(btn);
  }
}

/// Copy one block's source to the clipboard.
///
/// The text comes from `textContent`, never from re-parsing the highlighted
/// markup: syntect's <span>s and any `mark.hl` the reader has placed are
/// markup we would have to strip, and stripping it by hand is exactly the
/// "execute, don't escape" mistake tenet 4 exists to prevent. `textContent` is
/// the code the reader sees, with the marks contributing nothing.
///
/// Delivery goes through the existing `copy_to_clipboard` command (arboard,
/// text only) rather than `navigator.clipboard`, so it uses the same path as
/// "Copy path" and the send fallback, and does not depend on the webview's
/// clipboard permissions.
async function copyCodeBlock(pre, btn) {
  // syntect emits no <code>; the plain fallback does. Either way this is the
  // element holding the code text.
  const inner = pre.querySelector("code") || pre;
  // syntect opens with a newline after `<pre …>`, and a fenced block always
  // ends in one. Neither is part of the code.
  const text = (inner.textContent || "").replace(/^\n/, "").replace(/\s+$/, "");
  if (!text) return;
  try {
    await invoke("copy_to_clipboard", { text });
  } catch (e) {
    toast("Copy failed: " + String(e));
    return;
  }
  btn.dataset.copied = "1";
  clearTimeout(btn._copiedTimer);
  btn._copiedTimer = setTimeout(() => delete btn.dataset.copied, 1400);
  toast("Code copied");
}

/// Scroll to an element by id within #content, returning whether it was found.
///
/// Shared by same-document `#anchor` links and the fragment half of a
/// cross-file `other.md#anchor` link. Headings are the only things inside
/// #content carrying an id, so scanning them beats `querySelector("#" + id)`
/// twice over: a slug like `1-intro` (from `## 1. Intro`) is a valid *id* but
/// an invalid CSS *id selector*, and `querySelector("#1-intro")` throws rather
/// than returning null. Scoping to #content also stops a document's own
/// `#content` or `#tree` link resolving to the app's chrome.
function scrollToFragment(frag) {
  let id = frag;
  try { id = decodeURIComponent(id); } catch (_) {}
  const t = [...contentEl.querySelectorAll("[id]")].find((el) => el.id === id);
  if (t) t.scrollIntoView();
  return !!t;
}

// `normalizePath` and `insideRepo` live in `paths.js` — the tenet-4 guard for
// relative links and images, split out so `ui/paths.test.mjs` can drive it.

// ---- highlights ----------------------------------------------------------
// Above this many placeable highlights, flattening the document once beats
// walking it per highlight. Below it, a walk that stops at the first match wins
// — flattening a 2MB document costs ~4ms whether there is one quote or five
// hundred. Measured crossover in the Chromium harness is around 5.
const SCAN_THRESHOLD = 4;

// `applyHighlights` assumes a virgin DOM — it never removes what is already
// there, which is true for its original caller because `renderCurrent` has just
// written `contentEl.innerHTML`. Anything repainting the overlay *in place* has
// to run this first or every mark gets wrapped again, one `<mark>` deeper each
// time.
//
// `normalize()` is not optional: unwrap leaves adjacent text nodes behind, and a
// quote split across two of them is silently skipped by locateInNodes — so
// without this, repeated repaints progressively stop finding their own marks.
function clearHighlights() {
  for (const m of [...contentEl.querySelectorAll("mark.hl")]) unwrap(m);
  contentEl.normalize();
  staleRail.innerHTML = "";
}

/// Re-place the highlight overlay without re-rendering the document.
///
/// The correct external-repaint call is `renderCurrent({preserveScroll: true,
/// reanchor: false})` — but that is the full path `save_to_paint` measures
/// (IPC render, `innerHTML`, `interceptLinks`, `decorateCodeBlocks`), far too
/// expensive for an agent resolving six marks in a row when the source has not
/// moved. Only the marks changed, so only the marks are re-placed.
///
/// The find bracket is the same pairing `renderCurrent` uses, and mandatory for
/// the same reason: every stored find `Range` points into text nodes that
/// `clearHighlights` is about to join and `applyHighlights` about to split.
async function repaintHighlights() {
  if (!currentFile) return;
  invalidateFind();
  clearHighlights();
  applyHighlights(await invoke("get_highlights", { path: currentFile }));
  if (findQuery) findRecompute(false);
}

function applyHighlights(list) {
  staleRail.innerHTML = "";
  if (!list.length) return;

  const active = list.reduce((n, h) => n + (h.state === "stale" ? 0 : 1), 0);
  // Building a fresh TreeWalker per highlight and re-walking from the top of a
  // 105k-node document is where `apply_highlights` spent its ~350ms at 100
  // highlights. Below the threshold the per-highlight walk wins because it stops
  // at the first hit, where the flatten always reads the whole document.
  const doc = active > SCAN_THRESHOLD ? scanTextNodes(contentEl) : null;
  const placements = [];
  // Quotes the cheap walk could not hold in one text node. They are not failures
  // — see `placeAcrossNodes` — but they need the flattened view, and building it
  // here would be building it before the walk's own wraps have mutated the DOM.
  // So they wait.
  const crossNode = [];

  for (const h of list) {
    // The rail's one remaining tenant. `sent_at` is not read here and no longer
    // paints anything — see `addStaleChip` for what the rail is now for.
    if (h.state === "stale") { addStaleChip(h); continue; }
    const quote = h.quote.trim();
    if (!doc) {
      // Few enough to place as we go; wrapping can only disturb text nodes we
      // have already passed.
      if (!wrapByWalk(contentEl, quote, h.id, h.prior)) crossNode.push(h);
      continue;
    }
    const p = locateInNodes(doc, quote);
    if (p) placements.push({ ...p, id: h.id, prior: h.prior });
    else crossNode.push(h);
  }

  // Wrapping splits a text node, which invalidates every offset computed after
  // it — so apply back to front and nothing needs recomputing.
  placements.sort((a, b) => b.at - a.at);
  for (const p of placements) {
    const range = document.createRange();
    range.setStart(p.node, p.offset);
    range.setEnd(p.node, p.offset + p.length);
    wrapRange(range, p.id, false, p.prior);
  }

  // The rest, against a view of the DOM as it now stands. `doc` is deliberately
  // not reused: every wrap above split a text node it was built from.
  if (crossNode.length) placeAcrossNodes(scanTextNodes(contentEl), crossNode);
}

/// Paint the quotes that do not fit inside one text node.
///
/// This is the placement that used to be missing, and its absence was a
/// disappearing highlight rather than a cosmetic gap. The frontend sends
/// `getSelection().toString()`, so a reader who drags across a bolded phrase
/// stores the quote `lima mike november oscar` for the source `Kilo lima
/// **mike november** oscar` — three text nodes on screen. `wrapRange` on the live
/// selection paints it once, at creation, because a `Range` spans elements
/// happily; every *later* paint went through `wrapByWalk`/`locateInNodes`, which
/// only ever looked inside a single node, found nothing, and drew nothing. The
/// mark was still in the store and still on the stack — the badge counted it —
/// so what the reader saw was a highlight that survived being annotated and then
/// vanished the next time anything repainted: reopening the file, a save, a send.
/// Reproduced with four marks on the stack and one visible.
///
/// One `<mark>` per text-node slice, all sharing the id, rather than one `<mark>`
/// around the whole range. `Range.surroundContents` throws on a range that
/// crosses an element boundary and `wrapRange`'s `extractContents` fallback would
/// re-parent the `<strong>`'s contents into the mark — restructuring the rendered
/// document to draw on it, which `unwrap` could not then undo symmetrically.
/// Wrapping each slice in place leaves the tree exactly as pulldown-cmark built
/// it, and `<strong><mark>golf</mark></strong>` paints bold *and* highlighted.
///
/// Everything that consumes a mark already tolerates several per id:
/// `clearHighlights` unwraps every `mark.hl` it finds, and the click handler goes
/// through `closest`. `deleteHighlight` is the one that did not, and now uses
/// `querySelectorAll`.
function placeAcrossNodes(doc, highlights) {
  const segments = [];
  for (const h of highlights) {
    const quote = h.quote.trim();
    if (!quote) continue;
    const at = doc.text.indexOf(quote);
    if (at < 0) continue; // genuinely not on screen; the store keeps it
    const slices = segmentsIn(doc, at, quote.length);
    slices.forEach((s, i) => {
      // Which end of the run this slice is, so the seams between them can be
      // closed in CSS. `mark.hl` carries `padding: 0 1px` and `border-radius:
      // 2px`, which on three adjacent slices drew three rounded pills with gaps
      // between — one phrase reading as three separate marks, which is a
      // different and wrong statement about the document.
      const run = slices.length === 1 ? null : i === 0 ? "start" : i === slices.length - 1 ? "end" : "mid";
      segments.push({ ...s, id: h.id, prior: h.prior, run });
    });
  }
  // Back to front across the whole set, not per highlight: two quotes can share
  // a text node, and wrapping the earlier one first would move the later one.
  segments.sort((a, b) => b.at - a.at);
  for (const s of segments) {
    const range = document.createRange();
    range.setStart(s.node, s.offset);
    range.setEnd(s.node, s.offset + s.length);
    const m = wrapRange(range, s.id, false, s.prior);
    if (s.run) m.dataset.run = s.run;
  }
}

/// The text-node slices covered by `[at, at + length)` of the flattened text.
///
/// One entry per node touched, so a quote inside a single node yields exactly the
/// one slice `locateInNodes` would have returned — this is a generalisation of
/// that, not a second code path beside it.
function segmentsIn(doc, at, length) {
  const end = at + length;
  const out = [];
  for (let i = nodeIndexAt(doc.starts, at); i < doc.nodes.length; i++) {
    const nodeStart = doc.starts[i];
    if (nodeStart >= end) break;
    const node = doc.nodes[i];
    const from = Math.max(at, nodeStart);
    const to = Math.min(end, nodeStart + node.nodeValue.length);
    // A zero-width slice happens when the range ends exactly on a node boundary;
    // wrapping it would insert an empty `<mark>` that `normalize` cannot remove.
    if (to > from) out.push({ node, offset: from - nodeStart, length: to - from, at: from });
  }
  return out;
}

// Wrap the first occurrence of `quote` that lies within a single text node,
// stopping the walk as soon as it is found. Returns true if it was placed;
// false hands the quote to `placeAcrossNodes`, which does not need it to fit.
function wrapByWalk(container, quote, id, prior) {
  if (!quote) return false;
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  for (let node; (node = walker.nextNode()); ) {
    const idx = node.nodeValue.indexOf(quote);
    if (idx < 0) continue;
    const range = document.createRange();
    range.setStart(node, idx);
    range.setEnd(node, idx + quote.length);
    wrapRange(range, id, false, prior);
    return true;
  }
  return false;
}

// Flatten the rendered document into one string plus the text nodes behind it,
// so quotes can be found with a native string search instead of a DOM walk.
function scanTextNodes(container) {
  const nodes = [];
  const starts = [];
  const parts = [];
  let total = 0;
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  for (let n; (n = walker.nextNode()); ) {
    nodes.push(n);
    starts.push(total);
    parts.push(n.nodeValue);
    total += n.nodeValue.length;
  }
  return { nodes, starts, text: parts.join("") };
}

// First occurrence of `needle` that lies wholly within a single text node,
// found without re-walking the DOM. Occurrences straddling a node boundary are
// skipped, not failed — matching the previous per-node search exactly.
function locateInNodes(doc, needle) {
  if (!needle) return null;
  for (let at = doc.text.indexOf(needle); at >= 0; at = doc.text.indexOf(needle, at + 1)) {
    const i = nodeIndexAt(doc.starts, at);
    const node = doc.nodes[i];
    if (at + needle.length <= doc.starts[i] + node.nodeValue.length) {
      return { node, offset: at - doc.starts[i], length: needle.length, at };
    }
  }
  return null;
}

// Index of the last entry in the sorted `starts` that is <= `at`.
function nodeIndexAt(starts, at) {
  let lo = 0, hi = starts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid] <= at) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

/// The rail's only chip, and the only thing the rail is for.
///
/// It says one thing and it is now the only thing said there: this passage's own
/// text changed in the source and the anchor no longer finds it. Everything else
/// that used to queue up beside the document is gone — a passage that is out with
/// the agent raises nothing, because a question that has been asked is a question
/// dealt with, and a passage the renderer cannot draw raises nothing either,
/// because that is not a fact about the source. The rail is deliberately almost
/// always empty.
function addStaleChip(h) {
  const chip = document.createElement("div");
  chip.className = "stale-chip";
  chip.innerHTML =
    `<b>? still pertinent</b><span class="q">${escapeHtml(h.quote.slice(0, 120))}</span>`;
  const row = document.createElement("div");
  row.className = "row";
  const keep = document.createElement("button");
  keep.textContent = "Keep";
  keep.onclick = () => chip.remove();
  const drop = document.createElement("button");
  drop.textContent = "Dismiss";
  drop.onclick = async () => { await invoke("remove_highlight", { id: h.id }); chip.remove(); refreshStack(); };
  row.appendChild(keep); row.appendChild(drop);
  chip.appendChild(row);
  staleRail.appendChild(chip);
}

/// D3's pending chip and its "Answered" button used to live here.
///
/// Both are gone. A send stamps `sent_at` and takes the pair off the stack, and
/// that is now the end of it as far as the reader is concerned: a question handed
/// to the agent is assumed dealt with. The chip was one card per sent mark on the
/// rail, so a five-question send put five of them beside the paragraph they were
/// about, each asking to be clicked — and the click only ever recorded "dealt
/// with", which the send had already implied.
///
/// The store still keeps `sent_at`, and `Store::resolve` still clears it when the
/// agent calls `resolve_highlight` — the agent's record of what it has closed is
/// worth keeping and `list_highlights` filters on it. It simply no longer paints.

// How much rendered text either side of a selection is sent as context. The
// backend only compares the first/last 64 bytes of it; the rest is slack for
// the markdown syntax the DOM has thrown away.
const CONTEXT_CHARS = 96;

// The rendered text immediately before and after a selection.
//
// Without it a quote that appears twice in the file anchors to whichever copy
// comes first, not the one the reader selected — the quote alone cannot tell
// them apart. This is DOM text, so it has lost the markdown syntax the source
// still carries; `markdown::locate` scores it as a partial match rather than
// requiring it to line up exactly.
//
// It walks out node by node rather than taking a Range back to the start of
// the document, whose `toString()` would build a copy of the whole file.
function selectionContext(range) {
  const walker = document.createTreeWalker(contentEl, NodeFilter.SHOW_TEXT);
  const text = (n) => n.nodeValue || "";

  let prefix = "";
  if (range.startContainer.nodeType === Node.TEXT_NODE) {
    walker.currentNode = range.startContainer;
    prefix = text(range.startContainer).slice(0, range.startOffset);
    while (prefix.length < CONTEXT_CHARS && walker.previousNode()) {
      prefix = text(walker.currentNode) + prefix;
    }
  }

  let suffix = "";
  if (range.endContainer.nodeType === Node.TEXT_NODE) {
    walker.currentNode = range.endContainer;
    suffix = text(range.endContainer).slice(range.endOffset);
    while (suffix.length < CONTEXT_CHARS && walker.nextNode()) {
      suffix += text(walker.currentNode);
    }
  }

  return { prefix: prefix.slice(-CONTEXT_CHARS), suffix: suffix.slice(0, CONTEXT_CHARS) };
}

// `prior` is the fade `ui/theme.css` keys off. It is a *transient* flag — the
// Rust side sets it only on marks read off disk, and declares it
// `skip_serializing_if = "is_false"`, so the overwhelming majority of highlights
// arrive over IPC with no `prior` key at all. Absent therefore means false, and
// the coercion below is load-bearing: `data-prior="undefined"` is a present
// attribute and would fade every mark in the document.
function wrapRange(range, id, stale, prior) {
  const mark = document.createElement("mark");
  mark.className = "hl" + (stale ? " stale" : "");
  mark.dataset.id = id;
  if (prior === true) mark.dataset.prior = "";
  try {
    range.surroundContents(mark);
  } catch (_) {
    mark.appendChild(range.extractContents());
    range.insertNode(mark);
  }
  return mark;
}

function unwrap(mark) {
  const parent = mark.parentNode;
  if (!parent) return;
  while (mark.firstChild) parent.insertBefore(mark.firstChild, mark);
  parent.removeChild(mark);
}

function toggleHighlightMode(on) {
  highlightMode = typeof on === "boolean" ? on : !highlightMode;
  $("btn-hl-mode").classList.toggle("active", highlightMode);
  document.body.classList.toggle("hl-mode", highlightMode);
  toast(highlightMode ? "Highlight mode on" : "Highlight mode off");
}

async function triggerHighlight() {
  if (!currentFile) return;
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed) return;
  const quote = sel.toString();
  if (!quote.trim()) return;
  if (!contentEl.contains(sel.anchorNode)) return; // only within the preview
  const range = sel.getRangeAt(0).cloneRange();
  const { prefix, suffix } = selectionContext(range);
  const id = await invoke("add_highlight", {
    filePath: currentFile, quote, prefix, suffix,
  });
  // Minted this second, so never prior — the flag exists to say "read off disk".
  const mark = wrapRange(range, id, false, false);
  pending = { id, mark };
  sel.removeAllRanges();
  openAnnot({ mode: "create", quote, id });
}

// Clicking an existing dreamd highlight re-opens the annotation menu for editing.
async function openEditHighlight(id) {
  const hl = await invoke("get_highlight", { id });
  if (!hl) return;
  openAnnot({ mode: "edit", quote: hl.quote, id, text: hl.annotation || "" });
}

// ---- annotation modal ----------------------------------------------------
// `annotCtx` = { mode: "create"|"edit", id, quote }
let annotCtx = null;

function openAnnot({ mode, quote, id, text }) {
  annotCtx = { mode, id, quote };
  $("annot-title").textContent = mode === "edit" ? "Edit annotation" : "Add annotation";
  $("annot-save").textContent =
    // `displayField`: this button sits under the textarea it is a shortcut for,
    // so it must name the key that works *there*, modifier and all.
    (mode === "edit" ? "Save" : "Add to stack") + "  " + displayField(keymap.save_annotation);
  $("annot-delete").style.display = mode === "edit" ? "" : "none";
  $("annot-ev").textContent = quote;
  $("annot-text").value = text || "";
  $("annot-overlay").classList.add("open");
  setTimeout(() => $("annot-text").focus(), 0);
}
function closeAnnot() { $("annot-overlay").classList.remove("open"); annotCtx = null; }

async function saveAnnot() {
  if (!annotCtx) return;
  const text = $("annot-text").value.trim();
  if (!text) return;
  // set_annotation attaches the text AND (re-)enqueues the pair on the stack,
  // so editing a highlight that was removed from the stack puts it back.
  await invoke("set_annotation", { id: annotCtx.id, text });
  if (annotCtx.mode === "create") pending = null;
  closeAnnot();
  refreshStack();
  // The other half of the fade rule. `set_annotation` clears `prior`, so
  // re-annotating a faded mark is what brightens it — and without a repaint the
  // mark kept the wash it had while the panel below it showed the question back
  // on the stack, which is the two surfaces disagreeing about the same fact.
  //
  // It is also what puts a *new* mark on the store's terms rather than the
  // selection's: `triggerHighlight` wraps the live `Range`, which spans elements
  // happily, so a quote across a bold run painted once and then had to survive
  // `placeAcrossNodes` finding it again. Doing that here means a highlight that
  // cannot be re-placed is visibly wrong immediately, not on the next save.
  repaintHighlights();
}

async function cancelAnnot() {
  // Cancelling a brand-new highlight discards it; cancelling an edit keeps it.
  if (annotCtx && annotCtx.mode === "create" && pending) {
    await invoke("remove_highlight", { id: pending.id });
    unwrap(pending.mark);
    pending = null;
  }
  closeAnnot();
}

async function deleteHighlight() {
  if (!annotCtx || annotCtx.mode !== "edit") return;
  const id = annotCtx.id;
  await invoke("remove_highlight", { id });
  // `querySelectorAll`, not `querySelector`: a quote spanning a bold run or a
  // link is several `<mark>`s sharing one id (see `placeAcrossNodes`), and
  // unwrapping only the first left the rest of the passage painted for a mark
  // that no longer exists.
  for (const m of contentEl.querySelectorAll(`mark.hl[data-id="${id}"]`)) unwrap(m);
  closeAnnot();
  refreshStack();
}

// ---- stack panel ---------------------------------------------------------
// The stack is the ledger of the app's core loop, so this render path has one
// rule above every other: what the panel shows, in the order it shows it, is
// exactly what `get_stack` just returned. The animation below is in service of
// that, never at its expense.
//
// It *reconciles* rather than rebuilds. The old version wiped `innerHTML` and
// re-created every card on every change, which is correct but leaves no node
// alive long enough for a transition to attach to — and it silently re-checked
// every checkbox, so "Send selected" could only differ from "Send all" if you
// unchecked something in the seconds before pressing it. Keying each card by
// the highlight id fixes both: a pair that is still on the stack keeps its
// node (and therefore its checkbox), a new pair mounts with the enter
// animation, a departed one leaves with the exit animation.
//
// The hazard the exit animation introduces, and the reason for the two guards
// in `exitPair`/`checkedIds`: a leaving card is still in the DOM for ~170ms
// after its pair is gone from the store, and `send_stack(ids)` resolves ids
// against the *highlight* list rather than against the stack — so a checkbox
// that outlived its pair would put a just-removed pair back into the send.

// Monotonic, the same idiom as `paletteSeq`. Two refreshes can be in flight at
// once (a remove racing an annotation save); with stable keys an older reply
// landing last would resurrect a card that is already gone, where the old
// teardown-and-rebuild merely painted a stale list.
let stackSeq = 0;

// Mirrors the `.pair.leaving` transition in `index.html`, plus a little slack.
// Removal is on a timer rather than on `transitionend`: a card that outlives
// its pair is the one failure this file must not have, and `transitionend` can
// simply never arrive — close the panel or press Ctrl+M 50ms into an exit and
// the card is `display: none`, its transition abandoned. The timer does not
// care. (It also sidesteps `transitionend` firing once per property, which
// would otherwise cut the collapse short at whichever property finishes first.)
const STACK_EXIT_MS = 170;

// Motion is only ever attempted on a panel the reader can actually see. A
// closed (`display: none`) panel runs neither animations nor transitions, so
// animating into one buys nothing and would leave `.enter`'s `both` fill mode
// holding a card at `opacity: 0` until the panel is next opened. Reduced
// motion opts out for the same reason it always does.
function stackAnimates() {
  if (window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches) return false;
  if (document.body.classList.contains("view-mode")) return false;
  return $("stack-panel").classList.contains("open");
}

async function refreshStack() {
  const seq = ++stackSeq;
  const pairs = await invoke("get_stack");
  if (seq !== stackSeq) return; // superseded by a later refresh
  const badge = $("stack-badge");
  badge.textContent = pairs.length;
  badge.classList.toggle("show", pairs.length > 0);
  reconcileStack($("stack-list"), pairs);
}

// A live card: a `.pair` that still stands for something on the stack.
function isLivePair(n) {
  return !!n && !!n.classList && n.classList.contains("pair") && !n.classList.contains("leaving");
}

// Which ids are in a live submission, as of the refresh in progress. Computed
// once per reconcile and read by `updatePair`, which is called from two places
// in the same pass — rebuilding the set per card would be the same answer at a
// worse price.
let pendingNow = new Set();

function reconcileStack(list, pairs) {
  // Decided once per refresh, so an enter and an exit in the same pass cannot
  // disagree about whether this render is animated.
  const animate = stackAnimates();
  pendingNow = pendingSendIds();

  // The empty-state notice is removed up front so it can never be mistaken for
  // a position in the ordering pass below.
  const empty = list.querySelector(".empty");
  if (empty && pairs.length) empty.remove();

  // `.leaving` cards are deliberately not adoptable: a pair that comes right
  // back (annotate → remove → annotate) mounts a fresh card rather than
  // inheriting one mid-exit with its checkbox already torn out.
  const live = new Map();
  for (const el of [...list.children]) if (isLivePair(el)) live.set(el.dataset.id, el);

  const ordered = [];
  const seen = new Set();
  for (const p of pairs) {
    const id = String(p.highlight.id);
    seen.add(id);
    let el = live.get(id);
    if (el) updatePair(el, p);
    else {
      el = buildPair(p);
      if (animate) el.classList.add("enter");
    }
    ordered.push(el);
  }
  for (const [id, el] of live) if (!seen.has(id)) exitPair(el, animate);

  // Place the cards in `pairs` order, moving only the nodes actually out of
  // place. Re-inserting a node restarts its CSS animation, so a blanket
  // append-everything sweep would replay the enter snap on every card on every
  // refresh. `.leaving` cards hold their old slot until they collapse and are
  // stepped over rather than counted as a position.
  let cursor = list.firstChild;
  for (const el of ordered) {
    while (cursor && cursor !== el && !isLivePair(cursor)) cursor = cursor.nextSibling;
    if (cursor === el) cursor = el.nextSibling;
    else list.insertBefore(el, cursor); // a null cursor appends
  }

  // Appended last, so it sits *below* any card still animating out rather than
  // claiming the list is empty above one the reader can still see.
  if (!pairs.length && !list.querySelector(".empty")) {
    const msg = document.createElement("div");
    msg.className = "empty";
    msg.textContent = `No pairs yet. Select text and press ${displayCombo(keymap.highlight)}.`;
    list.appendChild(msg);
  }
}

function buildPair(p) {
  const id = String(p.highlight.id);
  const el = document.createElement("div");
  el.className = "pair";
  el.dataset.id = id;

  const top = document.createElement("div");
  top.className = "top";
  const cb = document.createElement("input");
  cb.type = "checkbox";
  cb.checked = true;
  cb.dataset.id = id;
  const loc = document.createElement("span");
  loc.className = "loc";
  // Shares `button.icon`/`button.danger` from index.html instead of the inline
  // `marginTop` this used to carry, and moves up into the header row where the
  // rest of the card's chrome already lives.
  const rm = document.createElement("button");
  rm.className = "icon danger rm";
  rm.textContent = "✕";
  rm.setAttribute("aria-label", "Remove from stack");
  rm.dataset.tip = "Remove from stack"; // `wireTooltips()` delegates on [data-tip]
  rm.onclick = async () => {
    await invoke("remove_pair", { id: p.highlight.id });
    // Two surfaces, because a pop changes two things. The panel loses the card,
    // and the passage itself fades: `Store::remove_from_stack` sets `prior`, and
    // without this repaint the mark stayed at full strength until something else
    // happened to redraw it — a save, a send, reopening the file — so the fade
    // arrived minutes later, attached to whatever the reader did next.
    refreshStack();
    repaintHighlights();
  };
  top.append(cb, loc, rm);

  const ev = document.createElement("div");
  ev.className = "ev";
  const an = document.createElement("div");
  an.className = "an";
  el.append(top, ev, an);

  // Drop `.enter` once it has played, so a later reorder of this node does not
  // replay it. The class is only ever added on a visible panel (see
  // `stackAnimates`), so the event does arrive; if a Ctrl+M mid-animation
  // cancels it, the class simply survives and the snap replays on the way back.
  el.addEventListener("animationend", () => el.classList.remove("enter"), { once: true });

  updatePair(el, p);
  return el;
}

// Everything a refresh can legitimately change about an existing card. The
// checkbox is pointedly *not* touched: it is the reader's selection, and the
// old rebuild resetting it on every unrelated refresh was the bug that made
// "Send selected" indistinguishable from "Send all".
function updatePair(el, p) {
  const h = p.highlight;
  const span = h.line_start === h.line_end ? `L${h.line_start}` : `L${h.line_start}-${h.line_end}`;
  const rel = h.file_path.replace(/^.*\//, "");
  const stale = h.state === "stale";
  // `textContent` throughout, so none of this needs `escapeHtml` at all — the
  // quote and the annotation are user text and never markup (tenet 4).
  el.querySelector(".loc").textContent = `${rel} · ${span}${stale ? " · ⚠ stale" : ""}`;
  el.querySelector(".ev").textContent = h.quote.slice(0, 200);
  el.querySelector(".an").textContent = p.annotation;
  el.classList.toggle("stale", stale);
  // D16's "the stack shows the pending send". The card stays put and dims
  // rather than leaving: nothing has been sent, so a cancel has to be able to
  // restore *this*, not rebuild it.
  el.classList.toggle("pending", pendingNow.has(String(h.id)));
}

// Snap a removed card out toward the panel edge while collapsing its box, then
// drop it. Idempotent: a second call on an already-leaving card does nothing.
function exitPair(el, animate) {
  if (el.classList.contains("leaving")) return;
  el.classList.remove("enter");
  el.classList.add("leaving");
  el.setAttribute("aria-hidden", "true");
  // Guard 1 of 2. The checkbox goes *now*, the moment the pair leaves the
  // stack, so it cannot be picked up by `checkedIds()` — or by anything else
  // that queries `#stack-list` — during the exit. Guard 2 is the `.leaving`
  // filter in `checkedIds()` itself.
  const cb = el.querySelector('input[type="checkbox"]');
  if (cb) cb.remove();

  if (!animate) { el.remove(); return; }
  // Lock the height the collapse animates from.
  el.style.height = `${el.offsetHeight}px`;
  requestAnimationFrame(() => {
    el.style.height = "0px"; // inline, to beat the inline height set just above
    el.classList.add("out");
  });
  setTimeout(() => el.remove(), STACK_EXIT_MS);
}

// The file tree's collapsed state is one class on <body> and nothing else —
// `#workspace` drops the sidebar column in CSS. Nothing is torn down, so this
// is a pure style flip and the tree survives being hidden.
function toggleTree() { document.body.classList.toggle("nav-collapsed"); }

function toggleStack() { $("stack-panel").classList.toggle("open"); refreshStack(); }
function closeStack() { $("stack-panel").classList.remove("open"); }

// ---- pane navigation -----------------------------------------------------
// Move the keyboard one pane left or right. The window is a row — sidebar,
// document, then whichever panels are docked on the right — and this walks it.
//
// Only *visible* panes are in the walk, which is what makes one pair of keys
// enough: with nothing open on the right, `pane_right` from the document has
// nowhere to go and says so, rather than focusing a panel the reader cannot
// see. Nothing here opens or closes anything — that is what the toggles are
// for, and a navigation key that also opened panels would have no way to mean
// "just move".

/// The panes, left to right, each with a test for whether it is on screen and a
/// way to hand it the keyboard. Functions rather than elements because the
/// answer changes with every toggle, and this list is consulted per keypress.
///
/// The sidebar and the document are real columns. The three on the right are
/// not all columns: `#stack-panel` and `#pty-pane` hold a slot each (the stack
/// insets by the pane's width when both are up), while `#outline-panel` is a
/// transient overlay floating at `right: 8px` *above* the stack rather than
/// beside it. It is in the walk anyway, because a panel the reader can see and
/// click is one the keyboard should be able to reach — and it is ordered
/// innermost of the three because it is the one drawn on top.
const PANES = [
  {
    id: "sidebar",
    shown: () => !document.body.classList.contains("nav-collapsed") &&
                 !document.body.classList.contains("view-mode"),
    focus: () => $("tree").focus(),
  },
  {
    id: "document",
    shown: () => true,
    focus: () => scrollEl.focus({ preventScroll: true }),
  },
  {
    id: "outline",
    shown: () => $("outline-panel").classList.contains("open"),
    focus: () => $("outline-panel").focus(),
  },
  {
    id: "stack",
    shown: () => $("stack-panel").classList.contains("open"),
    focus: () => $("stack-panel").focus(),
  },
  {
    id: "agent",
    shown: () => popoutOpen() || $("pty-pane").classList.contains("open"),
    focus: focusAgent,
  },
];

/// `PANES` index of the document, which is the answer to every "where am I"
/// question that has no better one. Derived rather than written as `2` so
/// reordering the row above cannot silently point it at a panel.
const DOC_PANE = PANES.findIndex((p) => p.id === "document");

/// Which pane the keyboard is in now, as an index into `PANES`.
///
/// By containment rather than identity: focus is usually on something *inside*
/// a pane — a tree row, the composer's textarea, the terminal's hidden helper —
/// and all of those should answer with the pane they sit in. Searched
/// right-to-left so a nested pane wins over the one it is drawn inside.
function currentPane() {
  const active = document.activeElement;
  if (!active) return DOC_PANE;
  for (let i = PANES.length - 1; i >= 0; i--) {
    const host = paneHost(PANES[i].id);
    if (host && host.contains(active)) return i;
  }
  // Focus on <body> — which is where it sits until something claims it —
  // means the reader has not been anywhere yet. Answer "the document", so a
  // first press moves off the thing they are reading rather than out of the
  // sidebar they are not looking at.
  return DOC_PANE;
}

function paneHost(id) {
  if (id === "sidebar") return $("sidebar");
  if (id === "document") return scrollEl;
  if (id === "agent") return popoutOpen() ? $("agent-popout") : $("pty-pane");
  return $(id === "outline" ? "outline-panel" : "stack-panel");
}

/// Hand the agent surface the keyboard, wherever it currently is.
///
/// The card and the dock share one `#agent-body` (they are one body in two
/// containers), so this asks the containers rather than the body. The card gets
/// focus on itself, which is the read-only state its own `i` key opens the
/// composer out of; the dock has no such step, so this goes straight to
/// whatever the reader would type into.
function focusAgent() {
  if (popoutOpen()) { $("agent-card").focus(); return; }
  // The terminal fallback. Note the way *out* of it is `toggle_pane` or Escape,
  // both claimed inside xterm — the global handler returns early on any key
  // aimed at a terminal, so these navigation keys cannot be among them.
  if (pty.term && !$("pty-pane").classList.contains("native")) { pty.term.focus(); return; }
  const input = $("agent-input");
  if (input && input.offsetParent) input.focus();
  else $("agent-log").focus();
}

/// Step `dir` panes and focus what you land on. Stops at the ends rather than
/// wrapping: a row of panels has a left and a right, and wrapping from the
/// agent back to the sidebar would read as a jump, not a step.
function focusPane(dir) {
  const shown = PANES.map((p, i) => (p.shown() ? i : -1)).filter((i) => i >= 0);
  const here = currentPane();
  const at = shown.indexOf(here);
  // `here` is always visible in practice — you cannot be focused inside a
  // hidden pane — but if it somehow is, treat the move as coming from the
  // document rather than doing nothing.
  const from = at >= 0 ? at : shown.indexOf(DOC_PANE);
  const next = shown[from + dir];
  if (next === undefined) return;
  PANES[next].focus();
  paintPaneFocus();
}

/// The focus ring. `:focus-visible` will not do this on its own: these panes are
/// reached by script, and a programmatic `.focus()` on a `tabindex="-1"`
/// container does not count as keyboard focus to the browser — so without an
/// explicit class the reader moves the keyboard somewhere with nothing on
/// screen saying where it went.
function paintPaneFocus() {
  for (const p of PANES) {
    const host = paneHost(p.id);
    if (host) host.classList.toggle("pane-focus", host.contains(document.activeElement));
  }
}

/// Send from inside the stack panel, which is the one send that also *closes*
/// the panel.
///
/// The two docks are the same strip of window — the stack panel is docked right
/// and so, by default, is the pane — so the queue giving way to the conversation
/// it just produced is a substitution rather than a second panel piling up
/// beside the first. It is also what the reader means: the point of pressing
/// Send here is to go and read the answer, and the queue that produced it is
/// about to be empty anyway.
///
/// Deliberately not what `#btn-send` in the titlebar does. That button is
/// reachable with the panel closed and is the same verb Ctrl+Enter is; closing a
/// panel the reader did not open from is a side effect, not a hand-off.
///
/// Closing first, and without awaiting: `runStack` opens the pane, and doing it
/// in this order means one reflow where the pane takes the space the stack was
/// holding, rather than a frame of both. Neither toggle is disabled by this —
/// `toggle_stack` brings the queue back with the pane still up, and
/// `toggle_pane` puts the pane away with the queue still up.
function sendFromStack(ids) {
  closeStack();
  return runStack(ids);
}

// Distraction-free reading: `body.view-mode` hides the titlebar, the sidebar
// and both side panels together. Same shape as `toggleTree` — one class, no
// teardown — and deliberately *additive*: it never writes `nav-collapsed` or a
// panel's `open` class, so exiting restores whatever chrome was there before.
// It is a plain toggle, not a mode that auto-exits: the palette and settings
// float above view mode and leaving them puts you back where you were.
//
// The toast is the discoverability cost of hiding the titlebar — there is no
// button left to click, so entering names both ways out.
function toggleView() {
  const on = document.body.classList.toggle("view-mode");
  if (on) toast(`View mode — ${displayCombo(keymap.toggle_view)} or Esc to exit`);
}

function exitView() { document.body.classList.remove("view-mode"); }

// Jump the reading pane to the ends of the document. `#content-scroll` is the
// scroller, not the window, so the browser's own Home/End would do nothing
// unless focus happened to be inside it — which is why these are worth binding
// at all. Instant, unlike the four motion keys below: `j` and `d` are travel,
// and travel is worth animating, but Home/End are teleports — the reader is
// asking to *be* somewhere, not to go there, and easing the full height of a
// long document is both slow and the one place scrolling can jank. Everything
// else that moves this pane (`scrollIntoView`, restoring `scrollTop` after a
// re-render) is instant for the same reason.
function jumpTop() { jumpTo(0); }
function jumpBottom() { jumpTo(scrollEl.scrollHeight); }

/// Put the reader at an exact offset, cancelling any glide first.
///
/// The cancel cannot be left to `stepGlide`'s hijack check, which notices the
/// scroller *moving*: a jump to where we already are moves nothing. Press `d`
/// and then Home at the top of a document and the check sees 0 before and 0
/// after, while the reader very much meant "no, stay at the top" — and the
/// glide would carry them down anyway. Intent is not recoverable from the
/// value, so the three callers that mean to own the position say so out loud.
///
/// The `scrollIntoView` callers are deliberately not routed through here: they
/// aim at an element rather than an offset, and in the only case the detector
/// misses — the target already being exactly in place — there is no
/// disagreement to resolve, because the reader is already looking at it.
function jumpTo(top) {
  endGlide();
  scrollEl.scrollTo({ top });
}

// Vim's motions over the same scroller, and bound to bare `j`/`k`/`d`/`u` in
// every key mode — a reader should never reach for a modifier to move down a
// page. They dispatch below the `isEditable` guard, so the find bar and the
// annotation box still get plain letters.
//
// The document has no caret to move, so these are pure scrolling: `j` is a line
// the way vim's is, `d`/`u` are *half* a viewport the way `Ctrl+D`/`Ctrl+U`
// are. Half rather than whole on purpose — it leaves a band of already-read
// text on screen to land on, which is what makes it a motion rather than a
// page turn.
//
// Smooth, via `glideBy` — see the block below for why that is a hand-rolled
// animation rather than `scrollTo({ behavior: "smooth" })`.
function scrollLine(dir) { glideBy(dir * lineHeight()); }
function scrollHalf(dir) { glideBy(dir * scrollEl.clientHeight / 2); }

// ---- the glide -----------------------------------------------------------
// Smooth scrolling for the four motion keys, and the reason it is ~40 lines
// instead of one `behavior: "smooth"` is **key repeat**. Holding `j` delivers
// keydowns at the OS repeat rate, and the native smooth scroller treats each
// one as a new gesture to re-aim at: the result stalls and surges rather than
// travelling. What a reader holding a key means is "keep going", so this
// accumulates a *target* and eases toward it. Ten presses in flight are one
// motion to a point ten lines further on, not ten motions fighting.
//
// Easing is exponential rather than a fixed-duration tween, which is what lets
// the target move mid-flight: velocity is a function of the remaining distance,
// so a target that grows simply speeds the glide up instead of restarting it.
// It also makes one line and half a screen feel like the same gesture at
// different sizes — same settling time, different speed.
//
// The whole thing is inert when nothing is moving: the rAF loop exists only
// between the first press and arrival.

/// Half-life of the remaining distance, in ms. The single feel knob: lower is
/// snappier. 40ms puts a one-line press perceptually done in ~5 frames while
/// still reading as motion rather than as a jump.
const GLIDE_HALF_LIFE = 40;
/// Minimum pixels per frame, so the tail of the exponential does not crawl the
/// last two pixels over a dozen frames — an arrival that is visibly *late*
/// costs more than the eased tail buys.
const GLIDE_FLOOR = 0.6;

let glide = null; // { target, expected, raf, last } while a glide is in flight

function maxScroll() {
  return Math.max(0, scrollEl.scrollHeight - scrollEl.clientHeight);
}

/// Scroll `delta` pixels, smoothly, accumulating onto any glide already running.
function glideBy(delta) {
  // A reader who asked the OS for less motion gets the jump. Checked per press
  // rather than cached, because the setting can change while the app is up.
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    endGlide();
    scrollEl.scrollTop = clampScroll(scrollEl.scrollTop + delta);
    return;
  }
  // Accumulate onto the target, not onto where we happen to be mid-flight —
  // that is what makes the tenth press of a held `j` land ten lines further on
  // rather than ten lines from wherever the animation had reached.
  const from = glide ? glide.target : scrollEl.scrollTop;
  const target = clampScroll(from + delta);
  if (glide) { glide.target = target; return; } // the running loop will re-aim
  // `expected` starts at where the scroller is *now*, not null: a jump landing
  // between this press and the first frame — Home, a link click, a mark jump —
  // has to be visible to the hijack check below, and a null would skip it for
  // exactly that one frame and then drag the reader back off the jump.
  glide = {
    target,
    expected: scrollEl.scrollTop,
    last: performance.now(),
    raf: 0,
  };
  glide.raf = requestAnimationFrame(stepGlide);
}

function clampScroll(top) {
  return Math.max(0, Math.min(top, maxScroll()));
}

function stepGlide(now) {
  if (!glide) return;
  const cur = scrollEl.scrollTop;

  // Somebody else moved the scroller — a wheel, `scrollIntoView` from the find
  // bar, `jumpTop`, a mark jump, `scrollTop` restored after a re-render. Their
  // move wins and this glide is abandoned. Comparing against what we ourselves
  // last wrote is what makes this cover every one of those without each having
  // to know the glide exists; the tolerance absorbs the engine's sub-pixel
  // rounding of a fractional `scrollTop`.
  if (Math.abs(cur - glide.expected) > 1.5) {
    endGlide();
    return;
  }

  // Re-clamp every frame: the document can grow under a glide (an image
  // decoding, a re-render), and a target past the new end would stall the loop
  // against the bottom instead of arriving.
  glide.target = clampScroll(glide.target);
  const gap = glide.target - cur;
  if (Math.abs(gap) < 0.5) {
    scrollEl.scrollTop = glide.target;
    endGlide();
    return;
  }

  // `dt` is clamped because a backgrounded window delivers one enormous frame
  // on return, and an unclamped step would teleport rather than glide.
  const dt = Math.min(now - glide.last, 64);
  glide.last = now;
  const k = 1 - Math.pow(0.5, dt / GLIDE_HALF_LIFE);
  let step = gap * k;
  const floor = GLIDE_FLOOR * (dt / 16.7);
  if (Math.abs(step) < floor) step = Math.sign(gap) * Math.min(Math.abs(gap), floor);

  const next = cur + step;
  scrollEl.scrollTop = next;
  // Read back rather than trusting `next`: the engine rounds, and the whole
  // hijack check above depends on this being what the scroller actually holds.
  glide.expected = scrollEl.scrollTop;
  glide.raf = requestAnimationFrame(stepGlide);
}

function endGlide() {
  if (!glide) return;
  cancelAnimationFrame(glide.raf);
  glide = null;
}

/// One line of the prose being read, from the stylesheet rather than a constant
/// — the whole point of tenet 5 is that a palette may set its own type metrics,
/// and a hard-coded 24px would drift from them. `normal` and any other
/// non-pixel answer fall back rather than becoming `NaN`, which would silently
/// stop `j` working at all.
function lineHeight() {
  const px = parseFloat(getComputedStyle(contentEl).lineHeight);
  return Number.isFinite(px) && px > 0 ? px : 24;
}

// ---- position frames -----------------------------------------------------
// A reading position is `{ path, top }` — a file and a scroll offset — and it
// is the unit both the mark and the jump history are built out of. Stored as an
// offset rather than a heading anchor because an offset restores the exact spot
// including mid-section, and a document with no headings has no anchor to
// restore to at all.
//
// In memory, and only in memory. Tenet 2: all of this dies with the process,
// and nothing here is ever written to disk.

function here() {
  return currentFile ? { path: currentFile, top: scrollEl.scrollTop } : null;
}

/// Go to a stored frame.
///
/// `openFile` resolves once the document is rendered *and its scroll reset to
/// 0*, so awaiting it is what makes the offset below land on a laid-out
/// document rather than being overwritten by the reset. Skipped when the frame
/// is in the file already open, which is the common case and would otherwise
/// re-render for nothing.
///
/// A stored offset is a pixel position, so it drifts if the file changed on
/// disk since the frame was taken. `scrollTo` clamps to the scrollable range on
/// its own, so a document that has since got shorter lands at its end rather
/// than failing.
async function restoreFrame(f) {
  restoring = true;
  try {
    if (f.path !== currentFile) await openFile(f.path);
  } finally {
    // Cleared before the scroll, not after: a render that throws must not leave
    // the flag set and silently stop the history recording for the rest of the
    // session.
    restoring = false;
  }
  jumpTo(f.top);
}

// ---- the mark ------------------------------------------------------------
// Vim's marks, cut down to one: `m` remembers where you are, `'` goes back.
// Explicit and user-placed — not the automatic history below.
//
// **One mark, not twenty-six.** Vim's letter argument is deliberately dropped.
// A letter buys a second bookmark at the cost of a modal leader key: `m` alone
// would do nothing visible, and the app would sit in a half-typed state waiting
// on a letter that decides whether the next keystroke is a mark name or the
// highlight shortcut. That machinery — a timeout, a repeat guard, a disarm on
// every overlay and focus path — was the largest piece of input state in the
// frontend, and it existed to serve a second bookmark nobody had asked for. One
// mark makes both keys ordinary single combos with immediate feedback, and
// leaves `pendingMark` as a thing this file no longer has.
//
// **Global across the repo, not per file.** dreamd shows one document at a time
// out of a whole repo, so the useful move is "bookmark the spot in the
// architecture doc, come back to it from wherever I've wandered to" — which is
// cross-file by definition. A per-file mark would be a bookmark you can only
// use once you have already navigated to the thing you were trying to navigate
// to.
let mark = null; // { path, top }

function setMark() {
  const f = here();
  if (!f) { toast("Nothing open to mark"); return; }
  mark = f;
  toast("Mark set");
}

async function jumpMark() {
  if (!mark) { toast("No mark set — press the set-mark key first"); return; }
  // A jump to the mark is itself a teleport, so it goes on the history like any
  // other. Captured before the move, as everywhere else.
  const from = here();
  await restoreFrame(mark);
  pushJump(from);
}

// ---- jump history --------------------------------------------------------
// Browser-style back and forward over reading positions. Where the mark is
// explicit and holds one place indefinitely, this is automatic and holds the
// last `JUMP_MAX` places you were teleported away from.
//
// **What counts as a teleport** is the whole design question, and the rule is:
// push a frame when something moved you somewhere you did not scroll to
// yourself. That is every `openFile` (a link, the tree, the palette, `]`/`[`,
// the mark), plus an in-document jump that actually found its target (a `#`
// link, an outline click). It is *not* wheel or keyboard scrolling, a
// `file-changed` re-render, or a theme switch. Vim's jumplist excludes `j`/`k`
// for the same reason: a stack that records passive scrolling fills with junk
// within a minute of reading.
//
// Cross-file moves push from inside `openFile`, which is a genuine funnel —
// every navigation in the app goes through it, so one guarded line there covers
// the tree, the palette, `]`/`[` and link clicks at once. In-document moves
// push at their two call sites instead, because `scrollToFragment` is also the
// second half of a cross-file section link: pushing inside it would record a
// second frame for one jump, and that frame would be the *new* document at
// offset 0, which is not a place anyone was.
const jumpBack = [];
const jumpFwd = [];
const JUMP_MAX = 64;

// True while a pop or a mark jump is in flight. `restoreFrame` calls `openFile`
// like everything else, and without this the arrival would push the departure
// back onto the stack you just popped it from.
let restoring = false;

/// Record `from` as a place worth coming back to. Null-safe, so a call site can
/// pass `here()` unconditionally and get a no-op when nothing was open.
function pushJump(from) {
  if (!from) return;
  jumpBack.push(from);
  // A ring, not a leak: a long reading session is thousands of navigations.
  if (jumpBack.length > JUMP_MAX) jumpBack.shift();
  // A new jump invalidates the forward branch — standard undo-stack semantics,
  // and the reason forward is the fiddlier half of the pair.
  jumpFwd.length = 0;
}

async function jumpHistory(dir) {
  const from = dir < 0 ? jumpBack : jumpFwd;
  const to = dir < 0 ? jumpFwd : jumpBack;
  const f = from.pop();
  // Silence would read as a broken keybind rather than an empty stack.
  if (!f) { toast(dir < 0 ? "No earlier position" : "No later position"); return; }
  const cur = here();
  if (cur) to.push(cur);
  await restoreFrame(f);
}

/// Drop every frame naming `path`. A pop into a file that no longer exists
/// paints an error block and consumes the frame, so the frames go when the file
/// does — from the watcher's `file-removed` and from the delete path alike.
function forgetPath(path) {
  for (const s of [jumpBack, jumpFwd]) {
    for (let i = s.length - 1; i >= 0; i--) if (s[i].path === path) s.splice(i, 1);
  }
  if (mark && mark.path === path) mark = null;
}

/// File → Open moved the root. Every frame is a path into the old repo, so
/// there is nothing here that survives the move.
function forgetAllPositions() {
  jumpBack.length = 0;
  jumpFwd.length = 0;
  mark = null;
}

// ---- find in document ----------------------------------------------------
// Vim's `/`, `n` and `N` over the one document that is open. This is *not* the
// v2 cross-file content index: nothing here touches `search.rs`, `nucleo` or
// the `SearchIndex`. It is frontend-only, in-memory, and dies with the process.
//
// **The haystack is the flattened *rendered* text, not the markdown source.**
// The source is not resident in the frontend and never has been — every
// `read_source` happens Rust-side and `render_markdown` returns HTML — so
// searching it would mean a new IPC surface or a per-keystroke round trip. The
// rendered path costs nothing extra instead: `scanTextNodes` is already built
// for `applyHighlights`, flattens the document once per render (~4ms at 2MB in
// the Chromium harness, relative signal only), and `nodeIndexAt` already turns
// an offset back into a `(node, offset)` pair. Per keystroke both approaches
// run the same string scan. Searching what the reader can actually see also
// drops two holes the source has: `[text](url)` syntax noise, and `te**s**t`,
// where a match on screen is invisible in source.
//
// **Matches are painted, not wrapped.** See the `::highlight` comment in
// index.html — DOM wrapping can split an existing `mark.hl` in two and durably
// corrupt session state the reader cannot see. Nothing here mutates the DOM.
const FIND_ALL = "dreamd-find";
const FIND_CUR = "dreamd-find-current";
// Bounds a pattern like `.` over a 2MB document. A guess.
const FIND_MAX_HITS = 2000;

const findBar = $("find-bar");
const findInput = $("find-input");
const findCountEl = $("find-count");
const findModeEl = $("find-mode");

let findIndex = null;    // { nodes, starts, text } | null — rebuilt per render
let findQuery = "";      // the last *committed* pattern, not what is in the box
let findRanges = [];     // Range[], in document order
let findCurrent = -1;
let findCapped = false;  // the scan hit FIND_MAX_HITS
let findWarned = false;  // the no-paint toast is shown once per session

// Safari 17.2 / WebKitGTK 2.44. Without it the bar still opens, still counts,
// and n/N still step — the search works, it is just not painted. Deliberately
// no DOM-wrapping fallback: that would trade a structural guarantee about the
// app's core loop for a visual on old WebKit, which is the wrong way round.
const findPaints = () =>
  typeof Highlight === "function" && !!(window.CSS && CSS.highlights);

/// The two `::highlight()` rules, present in `#find-css` only while a search is.
///
/// They are not in index.html's stylesheet because declaring them there is not
/// free: a `::highlight()` rule in the sheet made Chromium resolve highlight
/// styles across every text node, costing +27% on the forced layout after a 2MB
/// render with nothing ever registered (291ms -> 369ms; deleting just these two
/// rules restored it to 295ms — Chromium harness, relative signal only). A
/// reader who is not searching should not pay that on every render.
///
/// Both rules set a foreground as well as a background: this is the one place
/// in the app whose colours cannot come from the palette's own light/dark
/// blocks, so they have to be legible on either. `--find` / `--find-cur` let a
/// palette say otherwise, and the fallbacks mean one that has never heard of
/// them still renders (tenet 5).
const FIND_CSS =
  "::highlight(dreamd-find) { background: var(--find, #3a5a8c); color: var(--find-text, #ffffff); }\n" +
  "::highlight(dreamd-find-current) { background: var(--find-cur, var(--hl, #f2d16b)); color: var(--hl-text, #1a1a1a); }";

function findCssOn() { $("find-css").textContent = FIND_CSS; }

/// Take the paint rules away. **Call this while the ranges are still
/// registered** — that is the entire point, see `closeFind`.
function findCssOff() { $("find-css").textContent = ""; }

function openFind() {
  if (!currentFile) { toast("Nothing open to search"); return; }
  findCssOn();
  findBar.classList.add("open");
  findInput.focus();
  // The last pattern is left in the box as a convenience — `/` then Enter
  // repeats the previous search — but selected, so typing replaces it. It is
  // *not* re-run here: nothing paints until Enter.
  findInput.select();
  if (!findPaints() && !findWarned) {
    findWarned = true;
    toast("This webview cannot paint matches — the count and next/previous still work");
  }
}

/// Close the bar and take the search with it.
///
/// The bar being open is exactly the condition for matches being painted, and
/// this is the only way out of it: Escape and the ✕ button both land here. That
/// equivalence is the whole navigation model — there is no state where a reader
/// is looking at highlights with no visible sign of what produced them, and no
/// second command to learn for putting them out.
function closeFind() {
  // **Order is load-bearing, and this is the fix for a bug that survived two
  // attempts.** Removing the matches from the model — by `CSS.highlights.delete`
  // first, then by collapsing their ranges — left colour on screen that only
  // went when something else repainted that strip, so matches disappeared under
  // the cursor one at a time and a fragment could linger a line away from the
  // word it belonged to. Both of those ask the engine to derive a dirty region
  // from a *geometry* change, and it does not do so reliably.
  //
  // Withdrawing the style instead does not have to be derived: the ranges are
  // still registered and still painted, their style just became "no highlight",
  // which is the same path a theme switch takes and repaints exactly the regions
  // that were coloured. Only then is the model torn down — the dirty regions are
  // already recorded, so clearing afterwards cannot un-record them.
  findCssOff();
  findBar.classList.remove("open");
  // Focus has to leave the input or every bare-letter binding stays behind the
  // `isEditable` guard and the reader's next `n` types an `n`.
  findInput.blur();
  // Paint first, references after: `clearFindPaint` mutates the ranges the
  // engine is still tracking, so it has to run while they are still ours.
  clearFindPaint();
  findQuery = "";
  findRanges = [];
  findCurrent = -1;
  findCapped = false;
  findCountEl.textContent = "";
  findModeEl.textContent = "";
}

/// Everything gone, bar included. For the paths where the document itself is
/// withdrawn — the watcher removing the open file, File → Open moving the root.
function resetFind() {
  findInput.value = "";
  closeFind();
}

/// The two `Highlight` objects, registered once and thereafter *mutated*.
///
/// Not `CSS.highlights.set(...)` / `.delete(...)` per search, which is what this
/// shipped as first. Deleting the registry entry took the matches out of the
/// model but did not invalidate the pixels they had painted: after the bar
/// closed the blue stayed on screen and then vanished a match at a time, as a
/// click or a scroll happened to repaint that strip. Mutating a registered
/// highlight is the change the engine is obliged to notice.
let hlAll = null;
let hlCur = null;

function findHighlights() {
  if (hlAll) return true;
  if (!findPaints()) return false;
  hlAll = new Highlight();
  hlCur = new Highlight();
  CSS.highlights.set(FIND_ALL, hlAll);
  CSS.highlights.set(FIND_CUR, hlCur);
  return true;
}

/// Empty the match set. Kept as a *belt* to `findCssOff`, not as the mechanism:
/// collapsing each range asks the engine to derive a dirty region from a
/// geometry change, which cleared most matches but reproducibly left the odd
/// fragment behind. Withdrawing the style is what actually repaints; this runs
/// second and costs nothing, since the ranges are being discarded anyway.
///
/// Safe to call with the rules still installed — that is the mid-search path,
/// where a new Enter replaces the match set and the engine repaints on the
/// highlight-contents change, which it has always done correctly.
function clearFindPaint() {
  if (!hlAll) return;
  for (const r of hlAll) r.collapse(true);
  hlAll.clear();
  hlCur.clear();
}

/// Drop everything that points into the DOM. Called at the top of
/// `renderCurrent`, before the `innerHTML` write: a `Range` into a detached
/// node is a live object that silently scrolls nowhere, so leaving one behind
/// is a dead `n` rather than an error. `findCurrent` deliberately survives, so
/// a re-render under an open bar can clamp back to roughly where it was.
function invalidateFind() {
  clearFindPaint();
  findIndex = null;
  findRanges = [];
}

/// Compile a pattern to a `RegExp`. `literal` escapes every metacharacter, and
/// therefore cannot throw; the regex form can, and `findSpans` is what catches.
///
/// Smart case either way: an all-lowercase pattern is case-insensitive, any
/// uppercase makes it exact. Vim's `smartcase`, and the right default here.
///
/// Escaping the literal into a `RegExp` rather than using `indexOf` +
/// `toLowerCase()` is deliberate: case-folding both haystack and needle can
/// change string *length* for some Unicode (`İ`), which silently corrupts every
/// offset after it. The regex engine indexes the original string, so offsets
/// are always sound.
function findCompile(q, literal) {
  const src = literal ? q.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") : q;
  const flags = "g" + (/[A-Z]/.test(q) ? "" : "i");
  return new RegExp(src, flags);
}

const FIND_META = /[.*+?^${}()|[\]\\]/;

/// Find the spans for `q`, deciding for the reader whether it is a literal or a
/// regular expression. There is no toggle button, and this is why there does not
/// need to be one.
///
/// **Literal first; regex only where the literal finds nothing.** The rule is
/// chosen because it cannot silently give a wrong answer in either direction:
///
///   * `app.js` finds the literal `app.js` when the document contains it,
///     rather than quietly also matching `appXjs` — a plain-regex reading gets
///     that wrong, and the reader has no way to see that it did.
///   * `\bread\b`, `(one|two)` and `^#` occur literally in almost no prose, so
///     they fall through to being the regular expression the reader plainly
///     meant by typing them.
///   * A pattern that is not valid regex at all — a lone `(`, a stray `[` — is
///     simply a literal. There is no error state to design for, which is what
///     lets the bar drop the red "bad pattern" branch entirely.
///
/// The cost is one extra scan of the haystack for a pattern that turns out to
/// be a regex, which happens once per Enter rather than once per keystroke.
/// Which reading was used is reported in `#find-mode`, so the fallback is never
/// silent.
function findSpans(q, text) {
  const literal = findScan(findCompile(q, true), text);
  if (literal.length || !FIND_META.test(q)) return { spans: literal, mode: "" };
  try {
    return { spans: findScan(findCompile(q, false), text), mode: "regex" };
  } catch (_) {
    return { spans: literal, mode: "" }; // not a regex either — the 0 hits stand
  }
}

/// Every match as an `[start, end)` pair of offsets into the flattened text.
function findScan(re, text) {
  const out = [];
  re.lastIndex = 0;
  for (let m; (m = re.exec(text)); ) {
    // `a*` and `(?:)` match empty and would spin here forever.
    if (m[0].length === 0) { re.lastIndex++; continue; }
    out.push([m.index, m.index + m[0].length]);
    if (out.length >= FIND_MAX_HITS) break;
  }
  return out;
}

/// An offset span in the flattened text as a live `Range`. Unlike
/// `locateInNodes` this does *not* skip spans that straddle a text-node
/// boundary — that is the whole point, and the Custom Highlight API is what
/// makes painting one safe.
function findRange(idx, start, end) {
  const i = nodeIndexAt(idx.starts, start);
  const j = nodeIndexAt(idx.starts, end - 1);
  const r = document.createRange();
  r.setStart(idx.nodes[i], start - idx.starts[i]);
  r.setEnd(idx.nodes[j], end - idx.starts[j]);
  return r;
}

/// Rebuild the match set for `findQuery` against the DOM as it stands now.
///
/// `move` separates the two callers. Enter is a search: pick the match from
/// where the reader is looking and scroll to it. A re-render under an open bar
/// is not — the reader asked for nothing, so the index is rebuilt, the current
/// match clamped, and the pane left exactly where `renderCurrent` put it.
function findRecompute(move) {
  const prev = findCurrent;
  clearFindPaint();
  findRanges = [];
  findCurrent = -1;
  findCapped = false;
  findModeEl.textContent = "";
  if (!findQuery) { findCountEl.textContent = ""; return; }

  // Lazily, so a session that never presses `/` pays nothing for the flatten.
  if (!findIndex) findIndex = scanTextNodes(contentEl);
  const { spans, mode } = findSpans(findQuery, findIndex.text);
  findCapped = spans.length >= FIND_MAX_HITS;
  findModeEl.textContent = mode;
  findRanges = spans.map(([a, b]) => findRange(findIndex, a, b));
  if (!findRanges.length) { findCountEl.textContent = "0/0"; return; }

  findCurrent = move
    ? findFirstBelow()
    : Math.min(Math.max(prev, 0), findRanges.length - 1);
  paintFind(true);
  if (move) scrollToMatch();
}

/// The first match at or after the top of the visible pane, falling back to the
/// first match in the document — so Enter moves you forward from where you are
/// reading rather than throwing you back to the top of the file.
///
/// Binary, not linear: the ranges come out of a forward scan of the flattened
/// text, so they are in document order and their vertical positions are
/// monotonic. That turns up to `FIND_MAX_HITS` layout reads into about eleven.
function findFirstBelow() {
  const top = scrollEl.getBoundingClientRect().top;
  let lo = 0, hi = findRanges.length - 1, best = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (findRanges[mid].getBoundingClientRect().top >= top) { best = mid; hi = mid - 1; }
    else lo = mid + 1;
  }
  return best < 0 ? 0 : best;
}

/// `rebuildAll` is false when only the current match moved — `n` and `N` change
/// which range is yellow and nothing else, so re-adding all 2000 blue ones on
/// every step would be pure waste.
function paintFind(rebuildAll) {
  findCountEl.textContent =
    `${findCurrent + 1}/${findRanges.length}${findCapped ? "+" : ""}`;
  if (!findHighlights()) return;
  if (rebuildAll) {
    hlAll.clear();
    for (const r of findRanges) hlAll.add(r);
  }
  hlCur.clear();
  hlCur.add(findRanges[findCurrent]);
}

/// Commit what is in the box. **The only thing in this file that searches.**
///
/// Nothing runs on `input`. Painting every keystroke meant the highlight
/// flickered through the prefixes of the word being typed and the pane was
/// yanked to a new match mid-word — and the same accumulating-input path is
/// where the reported glitches lived. Searching on Enter makes the bar an
/// ordinary text field until the reader says otherwise, and deletes the
/// debounce, the per-keystroke scan and their two guessed constants with it.
///
/// Re-committing an unchanged pattern steps instead of re-searching, so holding
/// Enter walks the matches the way `n` does.
function commitFind(dir) {
  const q = findInput.value;
  if (q !== findQuery) {
    findQuery = q;
    findRecompute(true);
  } else if (findRanges.length) {
    stepFind(dir);
  }
  // Hand focus back to the document so `n` and `N` are keys again rather than
  // letters. The bar stays open: it is the visible sign that a search is live,
  // and closing it is what puts the highlights out.
  findInput.blur();
}

/// Step to the next/previous match, wrapping — the same choice `stepFile` makes,
/// and for the same reason: stopping at the end is indistinguishable from a
/// dead key.
function stepFind(d) {
  if (!findQuery) {
    toast(`No search — press ${displayCombo(keymap.find)} first`);
    return;
  }
  if (!findRanges.length) { toast("No matches"); return; }
  findCurrent = (findCurrent + d + findRanges.length) % findRanges.length;
  paintFind(false);
  scrollToMatch();
}

/// `#content-scroll` is the scroller, never the window. Instant, not smooth,
/// matching `jumpTop`/`jumpBottom` — and because `n` held down should step
/// rather than animate.
function scrollToMatch() {
  const r = findRanges[findCurrent];
  if (!r) return;
  const box = r.getBoundingClientRect();
  // A zero-height rect is a match inside a `display: none` subtree: there is
  // nowhere to scroll to, and moving would be a lie about where you are.
  if (!box.height) return;
  const host = scrollEl.getBoundingClientRect();
  scrollEl.scrollTop += box.top - host.top - scrollEl.clientHeight / 3;
}

function wireFind() {
  findInput.onkeydown = (e) => {
    // Bare and Shift only. `Ctrl+Enter` is `send_stack` and must not be
    // shadowed here; Escape is left to the global handler, which knows the
    // ordering against view mode.
    if (e.key !== "Enter" || e.ctrlKey || e.metaKey || e.altKey) return;
    e.preventDefault();
    commitFind(e.shiftKey ? -1 : 1);
  };
  $("find-next").onclick = () => stepFind(1);
  $("find-prev").onclick = () => stepFind(-1);
  $("find-close").onclick = () => closeFind();
}

// ---- contents / outline panel --------------------------------------------
// Built by walking the rendered DOM rather than as a side channel from Rust:
// the headings are already in the tree the render just produced, and they
// already carry the ids `markdown::Slugger` minted, so the whole outline is one
// `querySelectorAll` and no extra IPC.
//
// Live-update vs rebuild-on-open is settled as both. A render rebuilds the list
// only when the panel is *open*, and otherwise marks it dirty — so an open
// panel tracks `file-changed` the way every other surface does, and a closed
// one costs a boolean per render instead of a walk of a document nobody is
// looking at the outline of.
let outlineDirty = true;

function toggleOutline() {
  const open = $("outline-panel").classList.toggle("open");
  if (open && outlineDirty) buildOutline();
}

function closeOutline() {
  $("outline-panel").classList.remove("open");
}

// The panel floats over the reading pane and is meant to be transient (D21):
// it fades when the pointer is away — CSS, not JS — and any scroll of the
// reader closes it.
//
// A scroll rather than a timeout, deliberately: a timeout can strand a panel
// half-faded over a paragraph, and it has to pick a number that is wrong for
// somebody. A scroll is the reader having moved on, whether the movement came
// from the wheel or from the jump the panel was opened to make.
function wireOutline() {
  scrollEl.addEventListener(
    "scroll",
    () => { if ($("outline-panel").classList.contains("open")) closeOutline(); },
    { passive: true },
  );
}

function refreshOutline() {
  outlineDirty = true;
  if ($("outline-panel").classList.contains("open")) buildOutline();
}

function buildOutline() {
  outlineDirty = false;
  const list = $("outline-list");
  list.innerHTML = "";
  const heads = currentFile ? contentEl.querySelectorAll("h1, h2, h3, h4, h5, h6") : [];
  if (!heads.length) {
    list.innerHTML = `<div class="hint">${currentFile ? "No headings in this document." : "No document open."}</div>`;
    return;
  }
  // One fragment, one insertion: a long document carries hundreds of headings
  // and appending each to the live list would lay the panel out once per entry.
  const frag = document.createDocumentFragment();
  for (const h of heads) {
    const btn = document.createElement("button");
    btn.className = `oi l${h.tagName[1]}`;
    // `textContent`, never `innerHTML`: a heading may contain inline markup (and
    // by now a `mark.hl` or two), and the outline wants the text the reader
    // sees. It is also the only reason this needs no escaping.
    btn.textContent = h.textContent.trim();
    // The element, not its id — a jump that cannot miss, and one that still
    // works for a heading whose slug is not a valid CSS selector. It cannot
    // miss, so unlike the `#anchor` path the push is unconditional.
    btn.onclick = () => {
      pushJump(here());
      // Dismissed explicitly rather than left to the scroll listener: a jump
      // to the heading already on screen scrolls by nothing at all, and a
      // panel that survived that click would be the one case where the
      // gesture appeared not to have worked.
      closeOutline();
      h.scrollIntoView({ block: "start" });
    };
    frag.appendChild(btn);
  }
  list.appendChild(frag);
}

// `.leaving` cards are excluded: a pair removed from the stack lingers in the
// DOM for the length of its exit animation, and `send_stack` resolves ids
// against the highlight list rather than against the stack — so a checkbox that
// outlived its pair would send a pair the reader just took off. `exitPair`
// already deletes that checkbox; this is the second, cheaper guard.
function checkedIds() {
  return [...document.querySelectorAll('#stack-list .pair:not(.leaving) input[type="checkbox"]:checked')]
    .map((c) => c.dataset.id);
}

// ---- the flow: Ctrl+Enter, the undo window, the queue ---------------------
// D2 makes Ctrl+Enter one verb — open the pane, cold-start if needed, submit
// the stack — and D10 says an empty stack is just the first half of that. The
// tmux path is still here as `sendStackTmux`, on a keybind nothing sets by
// default (D6).
//
// Nothing below decides *what* to send or *whether* a send may go: `flow.rs`
// owns that, and owns it with no clock in it, because both timers live here.
// What this file adds is the two events that state machine cannot observe —
// the undo window elapsing, and the agent looking idle.

// **There is no undo window any more.** D16 delayed every send by five seconds
// so a regretted one could be taken back; that was measured against the wrong
// cost. The regretted send is rare and cheap — Escape interrupts the turn — and
// the five seconds were paid on every single send, which is what the loop
// actually feels. A submission is now armed the moment it is queued and goes as
// soon as the pane can take it. `#send-bar` still offers Cancel, but only for
// the seconds a submission genuinely spends waiting on a cold start.
//
// What survives from that design is `flow.rs`: the dedupe, and one submission
// taken at a time in the order they were pressed. Two Ctrl+Enters are still two
// turns — Claude Code's own composer queues a line typed mid-turn, which is why
// dreamd no longer needs to.

// How long the child must produce nothing before dreamd believes it has
// finished *starting*. Not an idle check any more: see `paneReady`.
const BOOT_QUIET_MS = 1500;
// One timer for everything: the arming, the boot watch and the release. It only
// runs while something is pending, and the steady state is nothing.
const FLOW_TICK_MS = 350;

const flow = {
  // token -> { ids, armed }. The mirror of `flow.rs`'s queue, held here only so
  // the bar can paint without an IPC round trip per tick.
  pending: new Map(),
  timer: null,
  busy: false,
  // `Date.now()` of the last byte the child produced, and 0 until it has
  // produced any. See `paneReady`.
  lastData: 0,
};

/// Ctrl+Enter. One verb.
///
/// The queue call goes first and the pane opens underneath it, so a submission
/// exists before the vendor bundle is parsed — which is what lets the send bar
/// say what is about to happen during a cold start instead of after it.
///
/// The explicit `flowTick` at the end is the whole of "no delay": on a warm
/// pane `openPane` resolves in a frame or two, the tick arms and releases in
/// the same turn, and the interval below never gets a chance to run. It exists
/// for the cold start, where the answer is genuinely "not yet".
async function runStack(ids) {
  let queued = null;
  try {
    queued = await invoke("queue_send", { ids: ids && ids.length ? ids : [] });
  } catch (e) {
    toast(String(e));
    return;
  }
  // Opened either way (D10): an empty stack is not an error and not a scolding,
  // it is the other thing this key does.
  //
  // `true` is the whole of `popout = "send"`: this is the one path that knows a
  // send is what opened the agent, and the setting exists because *this* answer
  // is one to read rather than a session to move into.
  const opening = openAgent(true);
  if (queued) {
    flow.pending.set(queued.token, { ids: queued.ids, armed: false });
    paintSendBar();
    refreshStack();
    startFlowTimer();
  }
  await opening;
  if (queued) await flowTick();
}

function startFlowTimer() {
  if (!flow.timer) flow.timer = setInterval(flowTick, FLOW_TICK_MS);
}

function stopFlowTimer() {
  if (flow.timer) { clearInterval(flow.timer); flow.timer = null; }
}

/// Arm everything, submit everything the pane will take, repaint.
///
/// Called on the interval *and* directly by `runStack`, which is why it is
/// re-entrant-safe rather than merely scheduled.
///
/// `flow.busy` because a tick is asynchronous and the interval is not: two
/// overlapping ticks could both call `take_send`. The Rust side would hand the
/// second one nothing (taking removes), so this is belt rather than braces —
/// but a redundant IPC call per 350ms while a turn runs is not free either.
async function flowTick() {
  if (!flow.pending.size) { stopFlowTimer(); paintSendBar(); return; }
  if (flow.busy) return;
  flow.busy = true;
  try {
    noteBootQuiet();
    for (const [token, p] of flow.pending) {
      if (p.armed) continue;
      p.armed = await invoke("arm_send", { token });
      // False means the token is gone underneath us — cancelled from another
      // path — so stop painting a send that no longer exists.
      if (!p.armed) flow.pending.delete(token);
    }
    // Everything, not one: `take_send` hands back the oldest each call, and two
    // submissions pressed a second apart are two lines typed a moment apart.
    // Claude Code queues the second itself if the first is still running, which
    // is the behaviour the old idle wait was approximating from outside.
    while (flow.pending.size && paneReady()) {
      if (!(await releaseSend())) break;
    }
  } catch (e) {
    console.error(e);
  } finally {
    flow.busy = false;
    paintSendBar();
  }
}

/// Has the child finished *starting*?
///
/// The one thing left of the old idle heuristic, and it answers a much narrower
/// question than that one did. A cold-started Claude Code emits its first bytes
/// within milliseconds and then spends a second or two drawing itself; typing
/// into that window loses the line entirely. A second and a half of complete
/// silence is the end of it — a working TUI redraws continuously, so quiet
/// means the first frame is done.
///
/// Latching, and that is the point: once a pane has been quiet once, it is
/// **permanently** ready, mid-turn included. dreamd no longer waits for the
/// agent to look idle before typing, because the composer accepts a line during
/// a turn and queues it. Reset by `startPaneProcess`, so a restart earns the
/// wait again.
function noteBootQuiet() {
  if (pty.settled || !pty.running || !flow.lastData) return;
  if (Date.now() - flow.lastData >= BOOT_QUIET_MS) pty.settled = true;
}

/// Can the pane take a line right now?
///
/// The terminal has to *look* settled before it can be typed at: `pty_spawn`
/// returns as soon as the shell exists, and a prompt written into Claude Code's
/// composer before it has drawn one goes nowhere. Hence `BOOT_QUIET_MS` and the
/// whole boot-quiet dance above.
///
/// The native surface has no such gap. `agent_spawn` returns with a child whose
/// stdin is a pipe, and a turn written to a pipe before the model is ready waits
/// in the pipe. There is nothing to guess at, so nothing here guesses.
function paneReady() {
  if (agent.running) return true;
  return !!(pty.running && pty.settled);
}

/// Hand the oldest armed submission to the pane, if Rust agrees there is one.
/// False when it handed back nothing, which is the caller's cue to stop asking.
async function releaseSend() {
  const res = await invoke("take_send");
  if (!res) return false; // nothing armed, or the pane is not up. Not an error.
  flow.pending.delete(res.token);
  const n = res.ids.length;
  toast(`Sent ${n} ${n === 1 ? "question" : "questions"} to the agent`);
  // The stack has just shrunk and every mark in it has just become pending, so
  // both surfaces are stale by exactly one event.
  await refreshStack();
  await repaintHighlights();
  return true;
}

/// Take it back. Nothing was sent, so there is nothing to reverse — the pairs
/// never left the stack and `sent_at` was never stamped. With the undo window
/// gone this is reachable only while a submission is genuinely waiting on the
/// pane: a cold start, or a child that has exited.
async function cancelSend(token) {
  let ok = false;
  try {
    ok = await invoke("cancel_send", { token });
  } catch (e) {
    console.error(e);
  }
  flow.pending.delete(token);
  paintSendBar();
  refreshStack();
  if (!ok) toast("That one has already gone");
}

/// The fallback for the boot watch: if the quiet never comes — a child stuck
/// mid-draw, a login shell that never exec'd `claude` — this is the way the
/// stack still gets sent. It bypasses `paneReady` and nothing else.
async function sendNow(token) {
  try {
    await invoke("arm_send", { token });
    const p = flow.pending.get(token);
    if (p) p.armed = true;
    await releaseSend();
  } catch (e) {
    toast(String(e));
  }
  paintSendBar();
}

/// Every id in a live submission, for the dimmed cards in the stack panel.
function pendingSendIds() {
  const set = new Set();
  for (const p of flow.pending.values()) for (const id of p.ids) set.add(String(id));
  return set;
}

function paintSendBar() {
  const bar = $("send-bar");
  bar.textContent = "";
  const rows = [...flow.pending.entries()];
  bar.classList.toggle("open", rows.length > 0);
  // Lifts the toast clear of the bar rather than letting the two overlap.
  document.body.classList.toggle("sending", rows.length > 0);
  if (!rows.length) return;

  // With the undo window gone, a row on screen at all means the pane could not
  // take the line — it is starting, or its child has exited. So the bar says
  // which, rather than counting down at something that is about to happen
  // anyway. On a warm pane it flashes for one frame or never paints at all.
  const why = pty.opening
    ? "starting Claude Code"
    : !pty.running
      ? "the pane is not running"
      : "waiting for Claude Code to finish starting";
  for (const [token, p] of rows) {
    const row = document.createElement("div");
    row.className = "send-row";
    const what = document.createElement("span");
    what.className = "what";
    const n = p.ids.length;
    const noun = n === 1 ? "question" : "questions";
    // `textContent` throughout — none of this is user text, but the stack panel
    // next door renders quotes the same way and the rule is worth keeping whole.
    what.textContent = `${n} ${noun} queued — ${why}`;
    row.appendChild(what);
    const go = document.createElement("button");
    go.textContent = "Send now";
    go.className = "primary";
    go.onclick = () => sendNow(token);
    row.appendChild(go);
    // Still the last moment to change your mind, and now the only one: a
    // submission that is visible here has not been written to the pty.
    const stop = document.createElement("button");
    stop.textContent = "Cancel";
    stop.onclick = () => cancelSend(token);
    row.appendChild(stop);
    bar.appendChild(row);
  }
}

/// The tmux path (D6): a temp file and `send-keys` into a pane running
/// `claude`, falling back to the clipboard. Unbound by default and absent from
/// the settings panel's action list — bind `keymap.send_stack_tmux` by hand to
/// compare the two when the pane misbehaves.
async function sendStackTmux(ids) {
  try {
    const res = await invoke("send_stack", { ids: ids || [] });
    toast(`${res.method}: ${res.detail}`);
  } catch (e) {
    toast(String(e));
  }
}

// ---- print / export to PDF -----------------------------------------------
// Export is the OS print dialog and its "Save as PDF" destination. No PDF
// crate, no bundled renderer, nothing added to the dependency tree: the whole
// feature is an `@media print` stylesheet (see the `#print-css` block in
// index.html) plus this one call.
//
// **Tenet 1 holds.** dreamd picks no path and writes no file — the dialog is
// the OS's, the destination is the user's, and if they aim it at somewhere
// inside the repo that is their save dialog doing what they told it. The tenet
// is about the app not mutating repo content on its own, and nothing here
// touches the markdown at all.
//
// **This goes through Rust, and that is not incidental.** `window.print()` is a
// no-op in WKWebView: WebKit routes it to the UI delegate's `_webView:
// printFrame:`, which wry does not implement, so the JS call returns having
// done nothing whatsoever on the one platform dreamd is built for. The Rust
// side calls `NSPrintOperation` directly. See `print_document` in main.rs.
//
// Nothing is closed or toggled first. Both side panels are absolutely
// positioned overlays and the print sheet hides them along with the rest of the
// chrome, unconditionally — so what comes out is the same document whatever was
// open on screen, view mode included.
async function printDocument() {
  // A blank page, or worse the "Select a markdown file" placeholder, is not
  // something to hand to a printer.
  if (!currentFile) { toast("Nothing open to print"); return; }
  try {
    await invoke("print_document");
  } catch (e) {
    toast(String(e));
  }
}

// ---- command palette -----------------------------------------------------
let paletteResults = [];
let paletteSel = 0;

function openPalette() {
  $("palette-overlay").classList.add("open");
  const input = $("palette-input");
  input.value = "";
  input.focus();
  runPalette("");
}
function closePalette() { $("palette-overlay").classList.remove("open"); }

// Monotonic id so a slow query that resolves after a faster later one can't
// paint stale results over them.
let paletteSeq = 0;

async function runPalette(q) {
  const t0 = perf.now();
  const seq = ++paletteSeq;
  const results = await invoke("fuzzy_search", { query: q });
  if (seq !== paletteSeq) return; // superseded by a later keystroke
  paletteResults = results;
  perf.span("ipc_fuzzy_search", t0);
  paletteSel = 0;
  const t = perf.now();
  renderPalette();
  perf.span("palette_render", t);
  perf.span("palette_keystroke", t0);
}

// The rendered rows, kept so moving the selection doesn't rebuild all 200.
let paletteRows = [];

function renderPalette() {
  const box = $("palette-results");
  box.innerHTML = "";
  paletteRows = paletteResults.map((n, i) => {
    const el = document.createElement("div");
    el.className = "pr" + (i === paletteSel ? " sel" : "");
    el.innerHTML = `<div>${escapeHtml(n.name)}</div><div class="rel">${escapeHtml(n.rel)}</div>`;
    el.onclick = () => { closePalette(); openFile(n.path); };
    box.appendChild(el);
    return el;
  });
}
function movePalette(d) {
  if (!paletteResults.length) return;
  // Two class writes instead of rebuilding every row: the arrow key used to
  // cost 5x a character keystroke for a change to one element's class.
  paletteRows[paletteSel]?.classList.remove("sel");
  paletteSel = (paletteSel + d + paletteResults.length) % paletteResults.length;
  const sel = paletteRows[paletteSel];
  if (sel) {
    sel.classList.add("sel");
    sel.scrollIntoView({ block: "nearest" });
  }
}

// ---- file options menu + delete ------------------------------------------
let fileMenuNode = null;
let pendingDeletePath = null;

// How close to an edge the ⋯ menu may sit when it has to be clamped there.
const MENU_INSET = 6;

/// Place the ⋯ menu against its button, and inside the window.
///
/// It is `position: fixed`, so nothing clips it — it was simply laid out past the
/// bottom edge. Below the button when there is room, flipped above it when there
/// is not, which for a tree that fills the sidebar is most files rather than a
/// corner case: every one in the lower half opened somewhere nobody can see.
///
/// `.open` goes on *first*. The menu is `display: none` until then, and an
/// undisplayed element measures zero — the flip needs its real height, and the
/// width it is clamped against is now the box's own rather than a hardcoded
/// guess at it.
function openFileMenu(anchorEl, node) {
  fileMenuNode = node;
  const m = $("file-menu");
  m.classList.add("open");
  const r = anchorEl.getBoundingClientRect();
  const below = r.bottom + 4;
  const above = r.top - 4 - m.offsetHeight;
  m.style.left =
    Math.max(MENU_INSET, Math.min(r.left, window.innerWidth - m.offsetWidth - MENU_INSET)) + "px";
  m.style.top =
    (below + m.offsetHeight <= window.innerHeight - MENU_INSET
      ? below
      : Math.max(MENU_INSET, above)) + "px";
}
function closeFileMenu() { $("file-menu").classList.remove("open"); fileMenuNode = null; }

async function copyFilePath() {
  if (!fileMenuNode) return;
  const path = fileMenuNode.path;
  closeFileMenu();
  try { await invoke("copy_to_clipboard", { text: path }); toast("Path copied"); }
  catch (e) { toast(String(e)); }
}

function askDeleteFile() {
  if (!fileMenuNode) return;
  pendingDeletePath = fileMenuNode.path;
  closeFileMenu();
  $("confirm-detail").textContent = pendingDeletePath;
  $("confirm-overlay").classList.add("open");
}
function closeConfirm() { $("confirm-overlay").classList.remove("open"); pendingDeletePath = null; }

async function doDeleteFile() {
  if (!pendingDeletePath) return;
  const path = pendingDeletePath;
  closeConfirm();
  try {
    await invoke("delete_file", { path });
    toast("Moved to Trash");
    if (currentFile === path) {
      currentFile = null;
      contentEl.innerHTML = `<div class="empty">File deleted.</div>`;
      staleRail.innerHTML = "";
    }
    // The watcher's file-removed event refreshes the tree/index.
  } catch (e) { toast(String(e)); }
}

// ---- events / wiring -----------------------------------------------------

// `marks-changed`, the agent push path. Payload is
// `{file_path: string | null, stack: bool}`; a null `file_path` means repo-wide.
//
// Coalesced, mirroring the watcher's own debounce, because the emitter is an
// agent rather than a pair of hands: resolving six marks in a row is six events
// inside a few milliseconds, and each one un-coalesced costs a `get_highlights`
// round trip plus a full overlay re-place. The two flags accumulate across the
// window, so a burst that touched both the open document and the stack still
// does each job exactly once.
const MARKS_COALESCE_MS = 80;
let marksTimer = null;
let marksWantPaint = false;
let marksWantStack = false;

function onMarksChanged(payload) {
  const file = payload ? payload.file_path : null;
  // Repo-wide always concerns the open document; a named file only when it is
  // the one on screen. Marks in a file nobody is looking at need no paint —
  // `openFile` will call `get_highlights` for it whenever it is opened.
  if (file == null || file === currentFile) marksWantPaint = true;
  if (payload && payload.stack) marksWantStack = true;
  if (marksTimer) return;
  marksTimer = setTimeout(async () => {
    marksTimer = null;
    // Read and clear together, before the first `await`: an event arriving
    // mid-flight has to open a *new* window rather than be dropped into the one
    // already draining.
    const paint = marksWantPaint;
    const stack = marksWantStack;
    marksWantPaint = false;
    marksWantStack = false;
    try {
      if (paint) await repaintHighlights();
      if (stack) await refreshStack();
    } catch (e) { console.error(e); }
  }, MARKS_COALESCE_MS);
}

// ---- embedded Claude Code pane -------------------------------------------
// A terminal docked under the reading pane, running `claude` in the repo the
// window is open on — so the agent it talks to reaches this repo's MCP socket
// and sees the same stack the badge is counting.
//
// Everything here is lazy. `xterm.js` is 289 KB of vendored script and
// `portable-pty` is a process; a session that never presses the key pays for
// neither. That is also the perf contract: a cold-start regression means
// something below started running at boot.

const pty = {
  term: null,        // the xterm.js Terminal, once the vendor bundle is in
  fit: null,         // its FitAddon
  vendor: null,      // the in-flight (or settled) vendor-load promise
  running: false,    // whether a child process is alive on the Rust side
  listening: false,  // whether the pty-data / pty-exit listeners are attached
  prefs: null,       // the last `agent_prefs` payload, once the pane has opened
  staged: null,      // a permission mode chosen but not yet restarted into
  // Whether the child has finished drawing its first frame. Latching; see
  // `noteBootQuiet`, which is the only thing that sets it.
  settled: false,
  // The model chip pressed in this session, or null while the session is on
  // whatever Claude Code itself chose. dreamd passes no `--model`, so null is
  // "unknown", not "default".
  model: null,
  // True across the whole of `openPane` — the vendor load and the spawn, not
  // just the spawn. Only the send bar reads it, and only so that a cold start
  // says "starting" rather than "not running", which is the same fact worded as
  // a failure.
  opening: false,
  // What `setPaneStatus` last said. Held here rather than read back off
  // `#pty-status`, because the pop-out has no header and has to be able to show
  // it while that element is inside a dock that is `display: none`.
  status: "",
};

/// How each `agent.permission_mode` reads in the header, and in the sentence
/// that warns what a restart costs. The values are the config spellings, so the
/// `<option>` value is what `set_config` is handed with no translation table in
/// between — `pty.rs` owns the mapping from these onto four literal commands.
const MODE_LABELS = {
  "default": "ask each time",
  "accept-edits": "accept edits",
  "plan": "plan only",
  "bypass-permissions": "no prompts",
};

/// Inject the vendored bundles, once. Resolves when `Terminal` and
/// `FitAddon` are on `window`.
///
/// Same-origin `<script src>`/`<link>` created at runtime, which the CSP
/// (`script-src 'self'`) allows exactly as it allows the tags in index.html —
/// what it forbids is inline script and remote origins, and this is neither.
/// See `ui/vendor/README.md` for why the tags are not simply in the document.
function loadTerminalVendor() {
  if (pty.vendor) return pty.vendor;
  pty.vendor = new Promise((resolve, reject) => {
    const css = document.createElement("link");
    css.rel = "stylesheet";
    css.href = "vendor/xterm.css";
    document.head.appendChild(css);

    const load = (src) => new Promise((ok, no) => {
      const s = document.createElement("script");
      s.src = src;
      s.onload = ok;
      s.onerror = () => no(new Error(`could not load ${src}`));
      document.head.appendChild(s);
    });
    // Sequential: the addon's UMD wrapper expects nothing of xterm at load
    // time, but the order is free and a future addon may not be so relaxed.
    load("vendor/xterm.js")
      .then(() => load("vendor/addon-fit.js"))
      .then(() => {
        // Named rather than assumed. A vendored upgrade that changes the UMD
        // shape fails here, on the first open, instead of somewhere deeper.
        if (!window.Terminal || !window.FitAddon || !window.FitAddon.FitAddon) {
          throw new Error("vendor/xterm.js loaded but exported no Terminal");
        }
        resolve();
      })
      .catch(reject);
  });
  return pty.vendor;
}

/// xterm paints its own colours, so hand it the palette's. Read from the
/// *applied* variables rather than parsed out of the stylesheet, for the reason
/// `applyTheme` gives: the engine has already done the cascade.
/// Six keys, and no ANSI palette. The sixteen indexed colours are left entirely
/// to Claude Code (D19): a diff is drawn in ANSI red and green, and a "reading
/// coloured" red is the one restyle here that could make a removed line look
/// like a kept one. Chrome is dreamd's, content is the TUI's.
function terminalTheme() {
  const v = (name, fallback) => appliedCssVar(name) || fallback;
  return {
    background: v("--sidebar-bg", "#100e17"),
    foreground: v("--fg", "#e8e4f3"),
    cursor: v("--accent", "#a48cf5"),
    // The glyph *under* the block cursor. Without it xterm inverts, which on an
    // accent-coloured cursor puts the accent's complement on screen.
    cursorAccent: v("--sidebar-bg", "#100e17"),
    selectionBackground: v("--hover", "#292435"),
    // The colour of a selection in an unfocused terminal — clicking into the
    // document to copy what the agent said is the common case, and xterm's
    // default for it is a grey that reads as "gone".
    selectionInactiveBackground: v("--hover", "#292435"),
  };
}

/// UTF-8 ↔ base64. The wire is base64 in both directions: output because a
/// 4 KiB read splits multi-byte characters (see `pty.rs`), input because a
/// paste is arbitrary bytes and `btoa` alone throws on anything above U+00FF.
const enc = new TextEncoder();
function toB64(text) {
  let s = "";
  for (const b of enc.encode(text)) s += String.fromCharCode(b);
  return btoa(s);
}
function fromB64(b64) {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/// The toggle, over whichever container the config gives the agent.
///
/// In `popout = "always"` the card *is* the pane: this key and the titlebar
/// button raise and lower it, and the dock is simply never used. That is the
/// setting's whole claim — one agent surface, not two to choose between at
/// every press.
function togglePane() {
  if (popoutOpen()) { lowerPopout(); return; }
  if ($("pty-pane").classList.contains("open")) closePane();
  else openAgent();
}

/// Show the pane, building the terminal and starting the process the first
/// time. Every later open is a class flip and a refit.
async function openPane() {
  const pane = $("pty-pane");
  // The card cannot keep a conversation the dock is about to show. `false`
  // because focus is on its way into this pane, not back to the document.
  if (popoutOpen()) lowerPopout(false);
  pane.classList.add("open");
  // The grid that docks the pane right is declared on `#main-wrap`, which
  // cannot see its child's class — see the `agent-right` block in index.html.
  document.body.classList.add("pane-open");
  pty.opening = true;
  try {
    // Ahead of everything, because it decides both the pane's geometry and
    // which of the two bodies it has. One round trip on first open only;
    // nothing here runs at boot.
    await loadAgentPrefs();
    if (nativeSurface()) {
      pane.classList.add("native");
      await openNativeAgent();
    } else {
      pane.classList.remove("native");
      await loadTerminalVendor();
      if (!pty.term) buildTerminal();
      fitPane();
      if (!pty.running) await startPaneProcess();
      pty.term.focus();
    }
  } catch (e) {
    console.error(e);
    setPaneStatus(String(e.message || e));
  } finally {
    pty.opening = false;
    paintSendBar();
  }
  // After the try, and outside it: a pane that failed to start is exactly the
  // pane whose MCP status is worth reading, and this must not be skipped by the
  // thing it explains.
  refreshMcpStatus();
}

/// Hide it. Deliberately *not* a kill: the scrollback and the conversation
/// survive behind `display: none`, and view mode hides the pane the same way
/// for the same reason. Escape lands here too (D12) — closing is hiding, and
/// only an explicit Ctrl+C to the child stops the process.
function closePane() {
  $("pty-pane").classList.remove("open");
  document.body.classList.remove("pane-open");
  contentEl.focus({ preventScroll: true });
}

/// The pane's config, read once per process on its first open — `agent.position`
/// and `agent.permission_mode` are both launch-time values, so there is nothing
/// to re-read while it is up. A failure leaves the defaults the markup already
/// carries rather than refusing to open a terminal over a config problem.
async function loadAgentPrefs() {
  if (pty.prefs) return pty.prefs;
  try {
    pty.prefs = await invoke("agent_prefs");
  } catch (e) {
    console.error(e);
    pty.prefs = { position: "right", permission_mode: "accept-edits" };
  }
  applyPanePosition(pty.prefs.position);
  $("pty-mode").value = pty.prefs.permission_mode;
  return pty.prefs;
}

/// Dock bottom or right. The refit is not decoration: the fit addon computes
/// rows and cols from the box it is in, and a pane that changed edge without
/// one would leave the child holding the old grid — wrapped lines, a status bar
/// drawn off the end, and box-drawing that no longer meets.
///
/// `fitPane` is called *and* the `ResizeObserver` will fire; both are wanted.
/// The observer is asynchronous and the reader would otherwise watch one frame
/// of stale grid, and a position change that happens to preserve the pixel size
/// fires no observer at all.
function applyPanePosition(position) {
  document.body.classList.toggle("agent-right", position === "right");
  fitPane();
}

function buildTerminal() {
  pty.term = new window.Terminal({
    // Monospace, and staying monospace (D18): Claude Code draws its boxes out
    // of `─│┌┐└┘` and they only meet on a fixed advance width.
    fontFamily: appliedCssVar("--font-mono") || "ui-monospace, Menlo, monospace",
    fontSize: 13,
    // A little air between rows, which is most of what separates a terminal
    // that reads as a document surface from one that reads as a console. Any
    // higher and the box-drawing verticals start to show gaps.
    lineHeight: 1.15,
    cursorBlink: true,
    // The child is a long-lived agent session; 5000 lines is what makes
    // scrolling back to what it did earlier useful rather than decorative.
    scrollback: 5000,
    theme: terminalTheme(),
  });
  pty.fit = new window.FitAddon.FitAddon();
  pty.term.loadAddon(pty.fit);
  pty.term.open($("pty-term"));
  // The way back out. xterm calls `stopPropagation` on every key it handles —
  // measured, in Chromium, with a capture-phase probe: `Escape`, `/`, `n`,
  // `Ctrl+M` and `Ctrl+T` all reach the document in the capture phase and none
  // of them in the bubble phase, which is where `wireKeys` listens. So the
  // global handler is not merely out-ranked here, it is never called, and
  // without this hook the pane would be a keyboard trap: the one key documented
  // to close it would be swallowed by the thing it closes.
  //
  // Returning false is xterm's "I handled it" — it must not also go to the
  // child, or `claude` would receive a stray ^T on the way out.
  //
  // Two keys now, not one, and the second one costs something real.
  //
  // Escape is Claude Code's *interrupt* — it is how you stop a turn you regret
  // mid-answer. D12 claims it for "close the pane" in every mode, so while the
  // pane has focus that interrupt is gone: the only way to stop a running turn
  // from in here is Ctrl+C, which in Claude Code is closer to "exit" than to
  // "cancel that thought". That is the price of Escape meaning one thing
  // everywhere in dreamd, and it was decided knowing it.
  //
  // Bare Escape only. A modified one is a different key and stays the child's,
  // which is what keeps `Alt+Escape`-style sequences intact.
  //
  // The compromise on the table and deliberately NOT built here: double-Escape,
  // where the first press goes to the child and a second within a short window
  // closes the pane. It is a feel question — the window length decides whether
  // it reads as responsive or as broken — and it needs a human at the keyboard,
  // not a guess in a diff.
  pty.term.attachCustomKeyEventHandler((e) => {
    if (e.type !== "keydown") return true;
    if (matchCombo(e, keymap.toggle_pane)) {
      e.preventDefault();
      togglePane();
      return false;
    }
    if (e.key === "Escape" && !e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      closePane();
      return false;
    }
    return true;
  });
  pty.term.onData((d) => {
    invoke("pty_write", { data: toB64(d) }).catch((e) => setPaneStatus(String(e)));
  });

  // Observing the box rather than the window: the pane's own drag handle
  // changes the terminal's geometry with no window resize involved, and a
  // dock switch changes it with neither. All three arrive here.
  new ResizeObserver(() => fitPane()).observe($("pty-term"));
  if (!pty.listening) {
    pty.listening = true;
    listen("pty-data", (e) => {
      // The whole of the boot watch's input. Stamped before the write rather
      // than after, so a slow `Terminal.write` cannot make the child look
      // quieter than it was — the direction that would type into a TUI still
      // drawing its first frame. See `noteBootQuiet`.
      flow.lastData = Date.now();
      if (pty.term) pty.term.write(fromB64(e.payload));
    });
    listen("pty-exit", (e) => {
      pty.running = false;
      pty.settled = false;
      $("pty-pane").classList.add("dead");
      const code = e.payload;
      setPaneStatus(code === null || code === undefined ? "exited" : `exited (${code})`);
      if (pty.term) pty.term.write("\r\n\x1b[2m[process exited — ⟳ to restart]\x1b[0m\r\n");
      // A submission waiting on this pane has just changed its reason for
      // waiting, and the bar is the only place that says what it is.
      paintSendBar();
    });
  }
}

async function startPaneProcess() {
  setPaneStatus("");
  $("pty-pane").classList.remove("dead");
  // Back to "has never spoken", which is what makes a queued send wait for the
  // new child to finish drawing rather than reading the *old* one's last byte
  // as evidence that this one is idle.
  flow.lastData = 0;
  // A new child has to earn the boot wait again — the old one's quiet says
  // nothing about this one's first frame.
  pty.settled = false;
  // And it comes up on whatever Claude Code chooses, not on the chip that was
  // lit: `/model` was typed into a process that no longer exists.
  pty.model = null;
  paintModelChips();
  const { rows, cols } = pty.term;
  // No mode on the wire. `pty_spawn` reads `agent.permission_mode` from the
  // config it already holds, which is why `commitModeChange` writes the config
  // *before* it restarts — see tenet 3 and `pty::pane_command`.
  await invoke("pty_spawn", { rows, cols });
  pty.running = true;
}

/// Re-measure and tell the child. Guarded on a visible box: a `ResizeObserver`
/// fires when the pane is hidden too, and `fit()` on a zero-height element
/// computes a nonsense geometry that the child would then be told about.
///
/// What `fit()` divides is `getComputedStyle(#pty-term).height` — the *border*
/// box in WebKitGTK — minus the padding of the `.xterm` div, which is why the
/// terminal's padding is declared there and not on `#pty-term`. See the
/// `#pty-term` block in index.html; this call is only correct because of it.
function fitPane() {
  const box = $("pty-term");
  if (!pty.fit || !box.clientHeight || !box.clientWidth) return;
  pty.fit.fit();
  if (pty.running) {
    invoke("pty_resize", { rows: pty.term.rows, cols: pty.term.cols }).catch(() => {});
  }
}

/// The pane's one line of status, and — since the pop-out has no header to put
/// it in — the card's too.
///
/// Kept on `pty` rather than read back off the element, because the card's hint
/// has to be able to ask for it while `#pty-status` is sitting in a dock that is
/// `display: none`. The interesting text here is a *failure*: `claude` not on
/// the login shell's PATH is the one thing that stops the surface working and
/// the one thing neither container can otherwise show. A card that answered
/// "starting…" forever would be reporting the failure as patience.
function setPaneStatus(text) {
  pty.status = text || "";
  $("pty-status").textContent = pty.status;
  paintPopout();
}

/// Kill and start again, in place. The button exists because the interesting
/// failure — `claude` not on the login shell's PATH — is one the user fixes
/// outside dreamd and then wants to retry without reopening anything.
async function restartPane() {
  try {
    if (nativeSurface()) {
      await invoke("agent_kill");
      agent.running = false;
      agent.busy = false;
      agent.turn = null;
      agent.blocks.clear();
      agent.tools.clear();
      // Cards belonging to the session that just died: their hooks are gone
      // and dreamd denied them on the way out, so leave them settled in the
      // log as the record of what was asked, and stop tracking them.
      agent.cards.clear();
      $("agent-log").replaceChildren();
      await openNativeAgent();
      return;
    }
    await invoke("pty_kill");
    pty.running = false;
    if (pty.term) pty.term.reset();
    fitPane();
    await startPaneProcess();
    pty.term.focus();
  } catch (e) {
    setPaneStatus(String(e.message || e));
  }
}

// ---- model ----------------------------------------------------------------
// Three chips in the header, and unlike the permission mode beside them this
// costs nothing: the model is a slash command Claude Code reads mid-session, so
// switching is a line typed into the running child and the conversation above
// it survives. Nothing is written to config — a model is a per-question choice
// ("ask haiku this one"), not a preference, and persisting it would mean every
// later session inheriting a decision made about one paragraph.
//
// dreamd never claims to know which model is live. It has not passed `--model`,
// the reader may have typed `/model` themselves, and Claude Code's own default
// moves. So no chip is lit until one is pressed, and a restart clears it again.

/// Switch the running session's model. The word crosses to Rust as a closed
/// enum and comes back out as one of three compiled-in lines (tenet 3) — see
/// `pty::model_line`.
async function setModel(model) {
  // Natively the slash command *is* a turn: verified against claude 2.1.220,
  // `/model haiku` sent as ordinary user-message text is honoured and the next
  // turn's init reports the new model. So the chips cost exactly what they cost
  // in the terminal — nothing — and the chip lights from that init rather than
  // from this click, which is the more honest of the two.
  if (agent.running) {
    try {
      await invoke("agent_send", { text: `/model ${model}` });
      $("agent-input").focus();
    } catch (e) {
      setPaneStatus(String(e.message || e));
    }
    return;
  }
  if (!pty.running) { setPaneStatus("the pane is not running"); return; }
  try {
    await invoke("pty_model", { model });
    pty.model = model;
    paintModelChips();
    // Focus follows the send: the reader's next act is almost always to type at
    // the agent, and a click on a chip would otherwise leave the caret in the
    // header.
    if (pty.term) pty.term.focus();
  } catch (e) {
    setPaneStatus(String(e.message || e));
  }
}

function paintModelChips() {
  for (const b of document.querySelectorAll("#pty-models .pty-model")) {
    b.classList.toggle("sel", b.dataset.model === pty.model);
  }
}

// ---- the MCP status line --------------------------------------------------
// The agent half of dreamd is invisible from the reading side. A reader who
// never ran `claude mcp add dreamd`, or whose window is the *second* one on a
// repo, gets an agent that answers questions perfectly well and cannot resolve
// a single mark — so the margin fills with pending glyphs for questions that
// were in fact answered, and nothing anywhere says why. This is the strip that
// says why.
//
// Read on open and on a repo change, and **not polled**. There used to be a
// five-second interval here, because the strip keyed on the client count and
// that could turn over at any moment. It no longer does — see `paintMcpStatus`
// — and everything left is fixed for a given root: whether a socket was armed,
// whether this window won the bind, and whether Claude Code's config names
// dreamd. A timer would re-ask three settled questions forever.
async function refreshMcpStatus() {
  let s = null;
  try {
    s = await invoke("mcp_status");
  } catch (e) {
    console.error(e);
    return;
  }
  // No answer is not the same as a bad one. An IPC that resolved to nothing
  // leaves whatever the strip already says, rather than accusing a healthy
  // socket of being unreachable on the strength of a missing reply.
  if (!s) return;
  paintMcpStatus(s);
}

/// Three failures, three sentences, and silence when there is nothing wrong.
///
/// **The client count is deliberately not one of the inputs.** The shim connects
/// per *tool call*, so a count of zero is equally true of a correctly-wired
/// agent that simply has not needed dreamd yet — which is most of most sessions.
/// The strip used to read that as "unregistered" and grow a Register button, and
/// because registering changed nothing it could observe, pressing it led to a
/// Restart that repainted Register. Whether an agent has got round to calling a
/// tool changes nothing about what it can do, so there is nothing to say.
///
/// **Nothing here has a button any more.** The native surface is handed
/// `--mcp-config` at spawn (`agent_spawn`) and cannot be unregistered, so the
/// third case below is unreachable for it. The surfaces that *can* be
/// unregistered are ones dreamd does not launch — a Claude Code in tmux, or the
/// terminal pane, whose command is four fixed literals with nowhere to put a
/// path — so the honest offer is the command, not a control. The other two cases
/// never had one: a second window cannot take a socket it lost, and no repo is a
/// File → Open.
function paintMcpStatus(s) {
  const el = $("pty-mcp");
  el.textContent = "";
  let text = null;
  let command = null;
  if (!s.armed) {
    text = "No repository open, so dreamd is not serving MCP — this agent cannot see your stack.";
  } else if (!s.serving) {
    text = "Another dreamd window owns this repository's MCP socket. This agent will reach that window's stack, not this one's.";
  } else if (!nativeSurface() && s.registered !== "yes") {
    // "unknown" is a `claude` dreamd could not run, which is a different
    // sentence: saying Claude Code does not know about dreamd would send the
    // reader to fix a registration when the binary is what is missing.
    text = s.registered === "unknown"
      ? "dreamd could not ask Claude Code whether it is registered. If this pane's agent never resolves a mark, this is the command that registers it:"
      : "Claude Code does not know about dreamd, so this pane's agent cannot see your stack. Run this once, for every repository:";
    command = s.command;
  }
  // Both containers, because the strip is moved between them and the rule that
  // shows it is scoped to whichever one currently holds it. Toggling only the
  // pane would make the warning invisible in `popout = "always"`, which is the
  // one mode where the pane never opens to show it.
  $("pty-pane").classList.toggle("mcp-warn", !!text);
  $("agent-popout").classList.toggle("mcp-warn", !!text);
  if (!text) return;
  const span = document.createElement("span");
  span.textContent = text;
  el.appendChild(span);
  if (!command) return;
  // `textContent`, so a launcher path is text and never markup — the same rule
  // tenet 4 applies to a document, applied to a path off the filesystem.
  const code = document.createElement("code");
  code.textContent = command;
  el.appendChild(code);
  const copy = document.createElement("button");
  copy.className = "pty-cta";
  copy.textContent = "Copy";
  copy.addEventListener("click", async () => {
    try {
      await invoke("copy_to_clipboard", { text: command });
      toast("Command copied");
    } catch (e) {
      setPaneStatus(String(e.message || e));
    }
  });
  el.appendChild(copy);
}

// ---- the native agent surface ---------------------------------------------
// The same pane, the same agent, drawn by dreamd. Where the terminal above is
// handed bytes and paints them itself, this is handed *events* — `wire::digest`
// has already turned Claude Code's stream-json into the few things worth
// showing — and turns them into DOM.
//
// **The prose goes through `markdown::to_html`**, the same pulldown-cmark and
// syntect the reader's documents go through. That one decision is what makes
// the answer look native rather than merely look different: same typeface, same
// palette, same syntax theme for fenced code, and it inherits tenet 4's
// escaping instead of needing its own.
//
// Text arrives twice and both are used. Deltas stream in as plain text so the
// reader sees prose move; the settled block then replaces that node wholesale
// with the rendered version. Rendering mid-stream would flicker between block
// types as fences and list markers arrive, and not rendering until the end
// would make a long answer look like nothing was happening.
//
// Blocks are keyed by the `index` the wire carries, and the key is scoped to the
// turn: indices restart at 0 for each assistant message, so a map that outlived
// a turn would have the second answer overwrite the first.

const agent = {
  /// The turn currently being written into, or null between turns.
  turn: null,
  /// index -> the element holding that block's text, for this turn only.
  blocks: new Map(),
  /// tool_use_id -> its ticker row.
  tools: new Map(),
  /// tool_use_id -> its permission card.
  cards: new Map(),
  running: false,
  listening: false,
  /// True while a turn is in flight, which is what the composer's Escape and
  /// the send button's disabled state both read.
  busy: false,
};

/// Whether the pane draws itself or hands the box to xterm.
///
/// Reads the prefs the pane already loads. Defaults to native when prefs failed
/// to load: that is the supported surface, and falling back to the terminal on
/// a config read error would be answering a question nobody asked.
function nativeSurface() {
  return (pty.prefs?.surface ?? "native") !== "terminal";
}

/// Show the native body, starting the session the first time.
///
/// Deliberately does **not** focus the composer. The terminal surface had to
/// take focus — an unfocused xterm is a dead box — but here the pane is a
/// reading surface with a text field at the bottom, and opening it is usually a
/// glance at what the agent is doing, not the start of typing. Stealing the
/// caret meant the toggle silently disarmed the reader's keys: `j`/`k`, `/`, the
/// highlight bindings all went into a textarea instead of the document. Opening
/// the pane opens the pane; clicking the composer is what says "I am typing".
async function openNativeAgent() {
  attachAgentListeners();
  if (!agent.running) {
    setPaneStatus("starting");
    await invoke("agent_spawn");
    agent.running = true;
    // Empty, not "ready". An idle pane says nothing — the lit dot beside the
    // title is already the answer to "is it alive", and a word that is true
    // almost all of the time is a word nobody reads. What is left in here is
    // only ever transient or bad news: "starting", "thinking", "stopped",
    // "exited (1)", an error.
    setPaneStatus("");
    $("pty-pane").classList.remove("dead");
  }
}

/// Attached once per process, guarded the way the pty listeners are: `listen`
/// returns an unlisten function nobody calls, so a second attach would double
/// every event.
function attachAgentListeners() {
  if (agent.listening) return;
  agent.listening = true;
  listen("agent-event", (e) => onAgentEvent(e.payload));
  listen("agent-ask", (e) => onAgentAsk(e.payload));
}

/// One digested event. The `kind` values are `wire::AgentEvent`'s variants —
/// change one, change the other.
function onAgentEvent(ev) {
  switch (ev.kind) {
    case "ready":
      // Emitted once per *turn*, not once per process, so this is "still here,
      // this is the model now" rather than "start a conversation". It is what
      // makes the model chips work without a restart.
      agent.model = ev.model;
      paintModelChipsFrom(ev.model);
      break;
    case "status":
      agent.busy = true;
      setPaneStatus(ev.status === "requesting" ? "thinking" : ev.status);
      paintComposer();
      break;
    case "textDelta":
      appendDelta(ev.index, ev.text);
      break;
    case "text":
      settleBlock(ev.index, ev.text);
      break;
    case "notice":
      addNote(ev.text);
      break;
    case "toolStart":
      addToolRow(ev.id, ev.name, ev.target);
      break;
    case "toolEnd":
      finishToolRow(ev.id, ev.ok);
      break;
    case "turn":
      endTurn(ev);
      break;
  }
}

/// The element a turn's content goes into, created on first use.
function turnEl() {
  if (agent.turn) return agent.turn;
  const wrap = document.createElement("div");
  wrap.className = "agent-turn agent";
  const who = document.createElement("div");
  who.className = "agent-who";
  who.textContent = "Claude";
  wrap.appendChild(who);
  $("agent-log").appendChild(wrap);
  agent.turn = wrap;
  return wrap;
}

function appendDelta(index, text) {
  let el = agent.blocks.get(index);
  if (!el) {
    el = document.createElement("div");
    el.className = "agent-said streaming";
    turnEl().appendChild(el);
    agent.blocks.set(index, el);
  }
  el.textContent += text;
  scrollLog();
}

/// Replace a streamed block with the rendered markdown of its settled text.
///
/// `render_agent_text` is the document pipeline, so this is where the agent's
/// answer stops being a terminal transcript and becomes a typeset one.
async function settleBlock(index, text) {
  let el = agent.blocks.get(index);
  if (!el) {
    el = document.createElement("div");
    turnEl().appendChild(el);
    agent.blocks.set(index, el);
  }
  // Hold on to the node: another turn may start before this round trip
  // returns, and `agent.blocks` will have been cleared by then.
  const node = el;
  try {
    // `innerHTML` of markdown::render_with's output — the same pipeline, and
    // therefore the same escaping, as every document in the reading pane: raw
    // HTML in the agent's reply is re-emitted as text and a link it invents
    // meets `paths.js` exactly as one in a file does. Tenet 4 covers this
    // string because it is the same string.
    node.innerHTML = await invoke("render_agent_text", { text });
    node.className = "agent-said";
  } catch (e) {
    console.error(e);
    // The text is the thing that matters; a render failure costs the
    // typesetting, not the answer.
    node.textContent = text;
    node.className = "agent-said streaming";
  }
  scrollLog();
}

function addNote(text) {
  const el = document.createElement("div");
  el.className = "agent-note";
  el.textContent = text;
  $("agent-log").appendChild(el);
  scrollLog();
}

function addToolRow(id, name, target) {
  const row = document.createElement("div");
  row.className = "agent-tool";
  const n = document.createElement("span");
  n.className = "t-name";
  n.textContent = name;
  const t = document.createElement("span");
  t.className = "t-target";
  t.textContent = target || "";
  const m = document.createElement("span");
  m.className = "t-mark";
  m.textContent = "…";
  row.append(n, t, m);
  turnEl().appendChild(row);
  agent.tools.set(id, row);
  scrollLog();
}

function finishToolRow(id, ok) {
  const row = agent.tools.get(id);
  if (!row) return;
  row.querySelector(".t-mark").textContent = ok ? "✓" : "✗";
  if (!ok) row.classList.add("failed");
}

/// A turn ended. Everything keyed by block index or tool id is scoped to the
/// turn and has to go with it — see the section header.
function endTurn(ev) {
  agent.blocks.clear();
  agent.tools.clear();
  agent.turn = null;
  agent.busy = false;
  // Blank on a clean finish, for the reason `openNativeAgent` blanks it: back to
  // idle is not news. An interrupt is.
  setPaneStatus(ev.interrupted ? "stopped" : "");
  if (ev.denials > 0) {
    addNote(
      ev.denials === 1
        ? "1 tool call was not allowed."
        : `${ev.denials} tool calls were not allowed.`,
    );
  }
  paintComposer();
  scrollLog();
}

/// A tool call waiting on the reader.
///
/// Its own event rather than a variant of the firehose above: it arrives rarely,
/// it is the only thing the reader has to *answer*, and routing it through the
/// same channel would mean filtering a hot path for a message that must never be
/// dropped.
function onAgentAsk(ask) {
  const card = document.createElement("div");
  card.className = "agent-card";

  const what = document.createElement("div");
  what.className = "c-what";
  what.textContent = `Claude wants to use ${ask.tool}`;

  const detail = document.createElement("div");
  detail.className = "c-detail";
  // textContent, not innerHTML: this is a tool's arguments, which for a Bash
  // call is a command line and for an Edit is file content. It is shown to the
  // reader as evidence, and evidence is never markup.
  detail.textContent = describeCall(ask.input);

  const buttons = document.createElement("div");
  buttons.className = "c-buttons";
  const allow = document.createElement("button");
  allow.className = "primary";
  allow.textContent = "Allow";
  allow.onclick = () => answerCard(ask.id, true, false);
  const always = document.createElement("button");
  always.textContent = `Always allow ${ask.tool}`;
  always.title = "For this conversation only — nothing is written to your config";
  always.onclick = () => answerCard(ask.id, true, true);
  const deny = document.createElement("button");
  deny.textContent = "Deny";
  deny.onclick = () => answerCard(ask.id, false, false);
  buttons.append(allow, always, deny);

  card.append(what, detail, buttons);
  $("agent-log").appendChild(card);
  agent.cards.set(ask.id, card);
  scrollLog();
}

/// The one line of a tool call worth putting on a card.
///
/// The whole input as pretty JSON is unreadable for the calls that matter most
/// — a Bash command is a string, and seeing it as `{"command": "..."}` is worse
/// than seeing it. So the conventional fields are shown bare and anything else
/// falls back to JSON.
function describeCall(input) {
  if (!input || typeof input !== "object") return String(input ?? "");
  for (const key of ["command", "file_path", "path", "pattern", "url", "query"]) {
    if (typeof input[key] === "string") return input[key];
  }
  try {
    return JSON.stringify(input, null, 2);
  } catch {
    return String(input);
  }
}

async function answerCard(id, allow, always) {
  const card = agent.cards.get(id);
  // Settle the card before the round trip: the reader has answered, and a
  // button that stays live for another 50ms invites a second click on a
  // decision they have already made.
  if (card) {
    card.classList.add("settled");
    const what = card.querySelector(".c-what");
    if (what) what.textContent += allow ? " — allowed" : " — denied";
  }
  agent.cards.delete(id);
  try {
    await invoke("agent_decide", { id, allow, always });
  } catch (e) {
    console.error(e);
  }
}

/// Send what is in the composer.
async function sendComposer() {
  const input = $("agent-input");
  const text = input.value.trim();
  if (!text || !agent.running) return;
  input.value = "";
  autoGrowComposer();
  addYourTurn(text);
  try {
    await invoke("agent_send", { text });
    agent.busy = true;
    paintComposer();
  } catch (e) {
    console.error(e);
    addNote(String(e.message || e));
  }
}

function addYourTurn(text) {
  const wrap = document.createElement("div");
  wrap.className = "agent-turn you";
  const who = document.createElement("div");
  who.className = "agent-who";
  who.textContent = "You";
  const said = document.createElement("div");
  said.className = "agent-said";
  said.textContent = text;
  wrap.append(who, said);
  $("agent-log").appendChild(wrap);
  // A new turn starts a new agent bubble.
  agent.turn = null;
  scrollLog();
}

/// Stop the turn without ending the conversation.
///
/// The terminal surface could not offer this: xterm.js claims every key it
/// handles and D12 spent Escape on "close the pane", leaving Ctrl+C as the only
/// way to stop a turn. Here dreamd owns the keyboard, so Escape interrupts while
/// a turn is running and closes the pane when one is not — the key does the more
/// urgent of its two jobs first.
async function interruptAgent() {
  try {
    await invoke("agent_interrupt");
  } catch (e) {
    console.error(e);
  }
}

function paintComposer() {
  const send = $("agent-send");
  if (send) send.disabled = !agent.running;
  setPaneDot(agent.busy);
  // The card's hint line is the pop-out's half of the same repaint: it says
  // what `#pty-status` and the dot say in the dock, and this is already the
  // function every status change and every turn boundary calls.
  paintPopout();
}

function setPaneDot(busy) {
  const dot = $("pty-dot");
  if (dot) dot.style.opacity = busy ? "1" : "";
}

/// Follow the tail only when the reader is already at it. Someone scrolled up
/// reading an earlier answer is *reading*, and yanking them down mid-sentence
/// is the rudest thing a log can do.
function scrollLog() {
  const log = $("agent-log");
  if (!log) return;
  const atBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 60;
  if (atBottom) log.scrollTop = log.scrollHeight;
}

function autoGrowComposer() {
  const input = $("agent-input");
  if (!input) return;
  input.style.height = "auto";
  input.style.height = `${input.scrollHeight}px`;
}

// ---- the pop-out (`agent.popout`) -------------------------------------------
// The same conversation as the dock, in a card centred on the window instead of
// an edge that spends the document's width. A dock is the right shape for a
// session you are working alongside and the wrong one for an answer you asked
// for in passing: the stack send in particular produces something to *read*,
// and reading it should not cost the page it is about.
//
// **One body, two containers.** `#agent-body` is moved between `#pty-pane` and
// `#agent-card` rather than duplicated — see `raisePopout` — so a turn that is
// mid-stream keeps streaming across the move, the tool ticker keeps its rows,
// and a permission card raised in one container is still there, still
// answerable, in the other. The MCP strip travels with it for the sharper
// reason: in `always` mode the dock never opens, so a strip left behind in it
// would be a warning nobody could ever be shown.
//
// The card is **read-only until asked**. Not as a restriction — the composer is
// one click or one `i` away — but because the default state of a thing floating
// over a document should be "readable", and a focused textarea in the middle of
// the window is a claim on the keyboard that an answer you glanced at has not
// earned. `Escape` gives the composer back, and a second one puts the card away.

const popout = {
  /// True while the composer is revealed. Every open returns to false.
  editing: false,
};

/// `never` | `send` | `always`, out of the prefs the pane already loads.
function popoutMode() {
  return pty.prefs?.popout ?? "never";
}

/// Should *this* open raise the card rather than the dock?
///
/// The terminal fallback never pops out, whatever the config says. xterm.js
/// needs a box whose size the fit addon manages and re-fits on every change;
/// this card changes height the moment a composer appears, and there is no
/// header in it to restart or re-mode the session from. `Surface::Terminal` is
/// a fallback for when the native surface cannot draw something — putting it in
/// the newer container would be the opposite of that.
function wantsPopout(fromSend) {
  if (!nativeSurface()) return false;
  const mode = popoutMode();
  return mode === "always" || (mode === "send" && fromSend);
}

function popoutOpen() {
  return $("agent-popout").classList.contains("open");
}

/// Does the keyboard belong to the card right now?
///
/// `contains` rather than an identity test, and it answers true for the card
/// itself: a node contains itself, and while the composer is up the caret is in
/// a descendant. That is what keeps `i` from being claimed out of the document.
function popoutHasFocus() {
  const card = $("agent-card");
  return !!(card && card.contains(document.activeElement));
}

/// Open the agent in whichever container the config asks for.
///
/// The one entry point, and the reason it is `async` before it decides
/// anything: `pty.prefs` is null until the first `agent_prefs` round trip, so a
/// first open that asked `wantsPopout` before awaiting it would read `never`
/// out of an empty object and dock — once per process, on exactly the open the
/// reader is most likely to be testing the setting with.
async function openAgent(fromSend = false) {
  await loadAgentPrefs();
  if (wantsPopout(fromSend)) return raisePopout();
  return openPane();
}

/// Raise the card, starting the session on the first open the way `openPane`
/// does.
async function raisePopout() {
  // One body, one container. The dock is closed *before* the move rather than
  // left open behind it: `closePane` after would find a pane whose conversation
  // had already gone and hide an empty box, and both open at once is a state
  // with no correct contents.
  if ($("pty-pane").classList.contains("open")) closePane();
  const card = $("agent-card");
  // The strip goes *inside* the body, above the composer, rather than under the
  // whole card: the composer is the bottom-most thing in every agent surface
  // dreamd draws, and a status line below the box you type into reads as
  // something the box produced.
  $("agent-body").insertBefore($("pty-mcp"), $("agent-composer"));
  card.insertBefore($("agent-body"), $("agent-hint"));
  $("agent-popout").classList.add("open");
  setEditing(false);
  // Focused on open, unlike the dock — which deliberately does not take focus,
  // because it opens *beside* what you are reading and stealing the caret there
  // disarms `j`/`k`/`/` for a pane you may only have glanced at. This card is
  // over the document rather than beside it, `i` is meaningless without focus,
  // and Escape hands the keyboard straight back. `j`/`k` still scroll, so the
  // focus costs the reader nothing it does not replace — see `wireKeys`.
  card.focus({ preventScroll: true });
  pty.opening = true;
  try {
    await openNativeAgent();
  } catch (e) {
    console.error(e);
    setPaneStatus(String(e.message || e));
  } finally {
    pty.opening = false;
    paintPopout();
    paintSendBar();
  }
  // Outside the try for the reason `openPane`'s copy is: a session that failed
  // to start is exactly the one whose MCP status is worth reading.
  refreshMcpStatus();
}

/// Put the conversation back in the dock and hide the card.
///
/// Hiding, never killing — the same promise `closePane` makes. The session, the
/// log and any unanswered permission card are all still there, in the dock,
/// which is where the next `openPane` will find them.
function lowerPopout(refocus = true) {
  $("agent-popout").classList.remove("open");
  setEditing(false);
  dockAgentBody();
  if (refocus) contentEl.focus({ preventScroll: true });
}

/// Return the travelling nodes to the pane, in the order the markup declares
/// them: head, MCP strip, staged-restart strip, terminal, body.
///
/// Order is not cosmetic here. `#pty-mcp` styles itself with a `border-bottom`
/// and sits above `#pty-confirm` so that a staged restart is the bottom-most
/// strip and reads as the newest thing; appended instead, it would land under
/// the terminal.
function dockAgentBody() {
  const pane = $("pty-pane");
  const body = $("agent-body");
  if (body.parentElement === pane) return;
  pane.insertBefore($("pty-mcp"), $("pty-confirm"));
  pane.appendChild(body);
}

/// Reveal the composer and take the caret, or give both back.
///
/// Two ways in, because the card is both something you read with a mouse in
/// your hand and something you reach without one: a click anywhere in it that
/// is not a button and not a text selection, and `i` while it has focus. Both
/// land here, so there is one state and one class.
function setEditing(on) {
  popout.editing = !!on;
  $("agent-popout").classList.toggle("editing", popout.editing);
  if (popout.editing) {
    const input = $("agent-input");
    input.focus();
    autoGrowComposer();
  }
  paintPopout();
}

/// Escape's two jobs in the card, urgent one first: give the composer back,
/// then put the card away.
///
/// Interrupting a running turn is a *third* job and outranks both, but it never
/// reaches here — the composer's own keydown handler claims Escape while
/// `agent.busy` and stops it propagating. So this is only ever the quiet case.
function escapePopout() {
  if (popout.editing) {
    setEditing(false);
    $("agent-card").focus({ preventScroll: true });
    return;
  }
  lowerPopout();
}

/// The card's one line of chrome, and it is a sentence rather than a control:
/// how to type, or what the agent is doing while it is doing it.
///
/// It stands in for the whole of the dock's header, which this card does not
/// have — the status text, the dot, and the "you can type here" that a visible
/// textarea says by existing. Hidden entirely while the composer is up, so the
/// card never shows both.
function paintPopout() {
  const hint = $("agent-hint");
  if (!hint) return;
  hint.textContent = "";
  hint.classList.toggle("busy", !!agent.busy);
  if (agent.busy) {
    hint.textContent = "thinking…";
    return;
  }
  if (!agent.running) {
    // Whatever `setPaneStatus` last said, which on the failing path is the
    // reason rather than the word "starting".
    hint.textContent = pty.status || "starting…";
    return;
  }
  hint.append("click or press ");
  const kbd = document.createElement("kbd");
  kbd.textContent = "i";
  hint.append(kbd, " to reply");
}

/// Light the chip matching whatever model the session reports.
///
/// The wire says `claude-haiku-4-5-20251001`; the chips say `haiku`. Substring
/// rather than a table, so a model renamed within its family still lights up.
function paintModelChipsFrom(model) {
  const name = String(model || "").toLowerCase();
  for (const chip of document.querySelectorAll("#pty-models .pty-model")) {
    chip.classList.toggle("sel", name.includes(chip.dataset.model));
  }
}

// ---- permission mode ------------------------------------------------------
// Claude Code reads its permission mode once, at launch, so changing it here is
// necessarily a restart — and a restart is a new session, which means the
// conversation in the scrollback is gone. That cost is the whole reason this is
// three functions instead of one `onchange`: the change is *staged*, the price
// is written on screen in a sentence, and it only happens if the reader says so.
//
// The preference is written back through `set_config`, the same path the
// settings panel and `dreamd config set` take, so a mode chosen here is the
// mode the next launch starts in (workshop 4.6).

/// Selecting a mode stages it. Nothing has been written or restarted yet.
function stageModeChange() {
  const next = $("pty-mode").value;
  const current = (pty.prefs && pty.prefs.permission_mode) || "accept-edits";
  if (next === current) { cancelModeChange(); return; }
  pty.staged = next;
  // A pane with no live child has no conversation to lose, so there is nothing
  // to warn about — write it and let the next start pick it up. True of either
  // surface: the mode is a launch flag both times.
  if (!pty.running && !agent.running) { commitModeChange(); return; }
  $("pty-confirm-text").textContent =
    `Switch to “${MODE_LABELS[next] || next}”? Claude Code reads the mode at launch, ` +
    `so this restarts the session and the conversation above is lost.`;
  $("pty-pane").classList.add("staging");
}

/// Put the select back and drop the staged mode. The select is the only thing
/// that moved, so this is the whole of the undo.
function cancelModeChange() {
  pty.staged = null;
  $("pty-pane").classList.remove("staging");
  if (pty.prefs) $("pty-mode").value = pty.prefs.permission_mode;
}

/// Write the preference, then restart into it. In that order, and it matters:
/// `pty_spawn` reads the mode out of the config rather than off the wire, so a
/// restart that ran first would come up in the *old* mode.
async function commitModeChange() {
  const mode = pty.staged;
  if (!mode) return;
  pty.staged = null;
  $("pty-pane").classList.remove("staging");
  try {
    await invoke("set_config", { patch: { agent: { permission_mode: mode } } });
    if (pty.prefs) pty.prefs.permission_mode = mode;
    if (pty.running || pty.term) await restartPane();
  } catch (e) {
    setPaneStatus(String(e.message || e));
    cancelModeChange();
  }
}

/// Is this event the terminal's?
///
/// Belt to `attachCustomKeyEventHandler`'s braces, and deliberately kept
/// despite being unreachable for every key xterm currently handles: xterm calls
/// `stopPropagation` on those, so they never arrive at `wireKeys` at all
/// (measured — see the comment on that hook). What *does* arrive is anything a
/// future xterm stops swallowing, and the answer for those must be "the child's,
/// not the app's" rather than whatever the global keymap makes of it.
function inTerminal(el) {
  return !!(el && el.closest && el.closest("#pty-pane"));
}

function wireEvents() {
  listen("file-changed", async (e) => {
    // `save_to_paint` is the core product loop: one :w in Neovim through to a
    // fully re-anchored, repainted document. `watcher_event` counts emissions
    // per save — anything above 1 is the missing debounce (fix B2).
    perf.at("watcher_event");
    if (e.payload && e.payload.path === currentFile) {
      const t0 = perf.now();
      // The file moved under us, so highlights genuinely have to be re-located.
      await renderCurrent({ preserveScroll: true, reanchor: true });
      perf.span("save_to_paint", t0);
    }
  });
  listen("file-added", async (e) => {
    // Most `file-added` events are an atomic-replace save of a file the tree
    // already has — Neovim's default `backupcopy=auto` and Claude Code's writer
    // both save that way, and the watcher cannot tell those from a genuinely
    // new file. Dropping the known ones here is what keeps `save_to_paint` off
    // the repo-walk path. Ahead of `perf.at`, so `events_per_save` keeps
    // counting events that caused work rather than events that arrived.
    if (e.payload && e.payload.path && knownPaths.has(e.payload.path)) return;
    perf.at("watcher_event");
    const t0 = perf.now();
    // `rebuild_index` hands back the tree from the walk it just did; asking for
    // it separately walked the whole repo a second time.
    paintTree(await invoke("rebuild_index"));
    perf.span("tree_rebuild", t0);
  });
  listen("file-removed", async (e) => {
    perf.at("watcher_event");
    const t0 = perf.now();
    paintTree(await invoke("rebuild_index"));
    perf.span("tree_rebuild", t0);
    // Before the early-out below, because a removed file is usually *not* the
    // one on screen and the stale frames pointing at it still have to go.
    if (e.payload && e.payload.path) forgetPath(e.payload.path);
    if (e.payload && e.payload.path === currentFile) {
      currentFile = null;
      contentEl.innerHTML = `<div class="empty">File removed.</div>`;
      staleRail.innerHTML = "";
      refreshOutline();
      resetFind();
    }
  });
  listen("theme-reloaded", () => loadTheme());

  // The store changed under us — an agent over the MCP socket, not this window.
  // Emitted *only* from that layer: a Tauri command's return value is already
  // the GUI's source of truth for its own mutation, so echoing user-origin
  // mutations back would put two repaint paths in a race against each other
  // (and `stackSeq` exists because two in-flight stack refreshes is a known
  // hazard). It also keeps `save_to_paint` entirely out of this path.
  listen("marks-changed", (e) => onMarksChanged(e.payload));

  // File → Open moved the tree root. Rust has already re-walked and re-read
  // config for the new repo by the time this fires, so this is the boot
  // sequence's data half over again — theme included, since a `.dreamd.toml`
  // in the new repo may name a different one. The payload is a file to open,
  // when the user picked a file rather than a folder.
  listen("repo-changed", async (e) => {
    try {
      forgetAllPositions();
      // A pattern survives a file change (vim's behaviour, and `renderCurrent`
      // re-runs it against the new document); it does not survive the repo
      // being swapped out from under it.
      resetFind();
      await loadTheme();
      await adoptRepoInfo();
      await loadTree();
      const file = e.payload;
      if (file) await openFile(file);
      else {
        currentFile = null;
        contentEl.innerHTML = `<div class="empty">Select a markdown file from the tree, or open the search palette.</div>`;
        staleRail.innerHTML = "";
        refreshOutline();
      }
      // `adopt_root` retired the old socket and bound one for the new root, so
      // both `armed` and `serving` may have just changed — a repo whose socket
      // another window already holds is the case that matters. The strip is not
      // polled, so without this it would keep asserting the previous repo's
      // answer for the rest of the session.
      refreshMcpStatus();
    } catch (err) { console.error(err); }
  });

  // Two sources for the same signal, on purpose. `matchMedia` is synchronous
  // and needs no IPC; `tauri://theme-changed` is the runtime-guaranteed one.
  // Whether WKWebView's prefers-color-scheme tracks the effective NSApp
  // appearance inside a Tauri window is the assumption the first is riding on —
  // the second is the belt to that braces. `applyAppearance` early-returns on
  // no change, so both firing costs nothing.
  prefersDark.addEventListener("change", (e) => applyAppearance(e.matches ? "dark" : "light"));
  listen("tauri://theme-changed", (e) => {
    const t = typeof e.payload === "string" ? e.payload : e.payload && e.payload.theme;
    if (t === "dark" || t === "light") applyAppearance(t);
  });
}

function wireUi() {
  $("btn-collapse").onclick = () => document.body.classList.add("nav-collapsed");
  $("btn-expand").onclick = () => document.body.classList.remove("nav-collapsed");
  // The two buttons stay one-way on purpose — each is only visible in the state
  // it acts on — so the keybind is the only caller that has to flip either way.
  $("btn-hl-mode").onclick = () => toggleHighlightMode();
  $("btn-pane").onclick = () => togglePane();
  $("pty-close").onclick = () => closePane();
  $("pty-restart").onclick = () => restartPane();
  for (const b of document.querySelectorAll("#pty-models .pty-model")) {
    b.onclick = () => setModel(b.dataset.model);
  }
  $("pty-mode").onchange = () => stageModeChange();
  $("pty-mode-go").onclick = () => commitModeChange();
  $("pty-mode-cancel").onclick = () => cancelModeChange();
  // The native composer. Enter sends and Shift+Enter newlines, which is why the
  // markup is not a <form>: that pairing is the opposite of a form's, and
  // intercepting the default is more code than owning the keydown.
  $("agent-send").onclick = () => sendComposer();
  $("agent-input").addEventListener("input", autoGrowComposer);
  $("agent-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendComposer();
      return;
    }
    // Escape does the more urgent of its two jobs: stop a running turn, or —
    // when nothing is running — close the pane, which is what it does
    // everywhere else. `stopPropagation` only in the first case, so the global
    // handler still sees the second.
    if (e.key === "Escape" && agent.busy) {
      e.preventDefault();
      e.stopPropagation();
      interruptAgent();
    }
  });
  // The pop-out's other way in. Two things are deliberately *not* a request to
  // type: a press that landed on a control — a permission card's Allow/Deny,
  // a code block's copy button — which is an answer or a copy and is complete
  // in itself, and a press that ends a text selection, which is reading. Both
  // would otherwise be swallowed by a textarea appearing under the cursor.
  $("agent-card").addEventListener("click", (e) => {
    if (popout.editing) return;
    if (e.target.closest("button, a, input, select, textarea")) return;
    const sel = window.getSelection();
    if (sel && !sel.isCollapsed && $("agent-card").contains(sel.anchorNode)) return;
    setEditing(true);
  });
  // In highlight mode, finishing a text selection auto-starts the flow.
  contentEl.addEventListener("mouseup", () => {
    if (!highlightMode || pending) return;
    const sel = window.getSelection();
    if (sel && !sel.isCollapsed && sel.toString().trim()) triggerHighlight();
  });
  // Click an existing highlight to edit / re-add / delete it.
  contentEl.addEventListener("click", (e) => {
    const m = e.target.closest && e.target.closest("mark.hl");
    if (m && contentEl.contains(m)) { e.preventDefault(); openEditHighlight(m.dataset.id); }
  });

  $("btn-outline").onclick = toggleOutline;
  $("outline-close").onclick = closeOutline;
  $("btn-stack").onclick = toggleStack;
  $("stack-close").onclick = () => closeStack();
  // Rightmost in the titlebar, and the primary action: the same verb Ctrl+Enter
  // is. It leaves the stack panel exactly as it found it — the reader pressing
  // this one did not ask to stop looking at the queue.
  $("btn-send").onclick = () => runStack([]);
  $("btn-copy").onclick = () => copyStack();
  // The panel's own two buttons hand over instead: see `sendFromStack`.
  $("btn-send-all").onclick = () => sendFromStack([]);
  $("btn-send-selected").onclick = () => sendFromStack(checkedIds());
  $("annot-save").onclick = saveAnnot;
  $("annot-cancel").onclick = cancelAnnot;
  $("annot-delete").onclick = deleteHighlight;
  // Submits the annotation straight from the textarea (keyboard-only flow).
  // The global handler can't do this: it bails on editable targets.
  // `matchField` for the reason the palette's next/prev use it: this is the box
  // the annotation is typed into, and `y` is a letter people write.
  $("annot-text").addEventListener("keydown", (e) => {
    if (matchField(e, keymap.save_annotation)) {
      e.preventDefault();
      saveAnnot();
    }
  });

  $("fm-copy").onclick = copyFilePath;
  $("fm-delete").onclick = askDeleteFile;
  $("confirm-cancel").onclick = closeConfirm;
  $("confirm-ok").onclick = doDeleteFile;
  // Close the file menu on any outside interaction.
  document.addEventListener("mousedown", (e) => {
    if (!e.target.closest("#file-menu") && !e.target.closest(".file-opts")) closeFileMenu();
  });

  const pin = $("palette-input");
  pin.oninput = () => runPalette(pin.value);
  pin.onkeydown = (e) => {
    // `matchField`, because this handler *is* a text field: the reader is
    // typing a query, and `n` and `p` are letters that belong in it. So these
    // two are Ctrl+N and Ctrl+P in every key mode, vim included — the arrows
    // beside them are the other way to say the same thing.
    if (matchField(e, keymap.palette_next) || e.key === "ArrowDown") { e.preventDefault(); movePalette(1); }
    else if (matchField(e, keymap.palette_prev) || e.key === "ArrowUp") { e.preventDefault(); movePalette(-1); }
    else if (e.key === "Enter") {
      e.preventDefault();
      const n = paletteResults[paletteSel];
      if (n) { closePalette(); openFile(n.path); }
    } else if (e.key === "Escape") { closePalette(); }
  };

  // click-away to close overlays
  $("palette-overlay").onclick = (e) => { if (e.target.id === "palette-overlay") closePalette(); };
  $("annot-overlay").onclick = (e) => { if (e.target.id === "annot-overlay") cancelAnnot(); };
  $("confirm-overlay").onclick = (e) => { if (e.target.id === "confirm-overlay") closeConfirm(); };
  $("settings-overlay").onclick = (e) => { if (e.target.id === "settings-overlay") closeSettings(); };

  wireFind();
  wireSettings();
  wireRootField();
  wireResizeHandles();
  wireOutline();
}

function isEditable(el) {
  if (!el) return false;
  // The pane is here rather than in a second guard of its own: everything a
  // bare-letter binding must stay out of is one predicate, and a terminal is
  // the most emphatic member of that set — every key in it belongs to the
  // child process. xterm's own focus target is a `<textarea>` and so already
  // matches the test below; this covers a click that lands on the viewport.
  if (inTerminal(el)) return true;
  return /^(input|textarea|select)$/i.test(el.tagName || "") || el.isContentEditable;
}

function wireKeys() {
  // The ring has to track focus however it moved, not only when a pane key
  // moved it — otherwise clicking into the stack leaves the ring on the
  // document and the next `pane_right` steps from somewhere the reader is not.
  // `focusin` rather than `focus` because it bubbles, and focus almost always
  // lands on something *inside* a pane.
  document.addEventListener("focusin", paintPaneFocus);

  document.addEventListener("keydown", (e) => {
    // The pane's keys are the child's, including every combo the branches below
    // claim. dreamd keeps exactly two — `toggle_pane` and Escape (D12) — and
    // both are claimed inside xterm rather than here: see
    // `attachCustomKeyEventHandler` in `buildTerminal` for why this branch
    // cannot be where that happens, and what claiming Escape costs.
    if (inTerminal(e.target)) return;
    if (e.key === "Escape") {
      // Recording a keybind swallows Escape as "cancel", not "close panel".
      if (cancelRecording()) { e.preventDefault(); return; }
      // Escape is also the way out of view mode, because view mode hides the
      // titlebar and leaves nothing to click. It is the *last* claim on the
      // key though: if any overlay or the file menu is open, this Escape
      // closes that and view mode survives.
      // `find-bar` is in the list but *not* in the overlay guard below: it is a
      // focused `<input>`, so `isEditable` already keeps every bare-letter
      // binding away from it — including `/` itself, which is what makes typing
      // a literal slash into the pattern work with no new code. Being here is
      // only what stops Escape-out-of-find also leaving view mode.
      //
      // One line covers the whole feature, because the bar being open is
      // exactly the condition for matches being painted: Escape closes the bar
      // and the highlights go with it. There is no separate `:nohlsearch` state
      // to arbitrate, and no way to be left looking at highlights with nothing
      // on screen to explain them.
      // `pty-pane` is in the list, but this branch only ever sees an Escape
      // pressed *outside* the terminal — one pressed inside it is claimed by
      // `attachCustomKeyEventHandler` and never reaches here. Both paths end at
      // `closePane`, which is what makes D12 one rule rather than two: Escape
      // closes the pane in every mode, and it hides rather than kills, so the
      // session is still there on the next open.
      // `agent-popout` is in the list and claims Escape the same way the dock
      // does, with one difference: its handler is a *step* rather than a close.
      // A card with the composer up gives the composer back first, because the
      // state the reader is escaping from is "typing", not "the card is here" —
      // and pressing it again does close the card. Interrupting a running turn
      // outranks both and never reaches this handler; the composer's own
      // keydown claims that one.
      const claimed = ["palette-overlay", "annot-overlay", "confirm-overlay",
                       "settings-overlay", "file-menu", "find-bar", "pty-pane",
                       "agent-popout"]
        .some((id) => $(id).classList.contains("open"));
      closePalette();
      closeFileMenu();
      if (popoutOpen()) escapePopout();
      if ($("pty-pane").classList.contains("open")) closePane();
      if (findBar.classList.contains("open")) closeFind();
      if ($("annot-overlay").classList.contains("open")) cancelAnnot();
      if ($("confirm-overlay").classList.contains("open")) closeConfirm();
      if ($("settings-overlay").classList.contains("open")) closeSettings();
      if (!claimed) exitView();
      return;
    }
    // While an overlay is open, let its own inputs handle keys.
    if ($("palette-overlay").classList.contains("open") ||
        $("annot-overlay").classList.contains("open") ||
        $("confirm-overlay").classList.contains("open") ||
        $("settings-overlay").classList.contains("open")) return;

    // ---- inside a text field ------------------------------------------
    // A focused field owns its letters, full stop. Only these five actions
    // survive one, and only in their *modified* spelling — `matchField` rather
    // than `matchCombo`, so `vim` mode's bare forms cannot reach in here and
    // turn `f`, `t` and `,` into commands while the reader is typing them.
    //
    // They survive because each is a place you go to rather than something you
    // do to the document: the palette, the settings panel, the agent pane, and
    // the two pane keys. Pane navigation in particular has to work from here or
    // it is a one-way door — focusing a pane usually means focusing the thing
    // in it you type into, and there would be no way back out.
    if (isEditable(e.target)) {
      if (matchField(e, keymap.palette)) { e.preventDefault(); openPalette(); return; }
      if (matchField(e, keymap.settings)) { e.preventDefault(); openSettings(); return; }
      if (matchField(e, keymap.toggle_pane)) { e.preventDefault(); togglePane(); return; }
      if (matchField(e, keymap.pane_left)) { e.preventDefault(); focusPane(-1); return; }
      if (matchField(e, keymap.pane_right)) { e.preventDefault(); focusPane(1); return; }
      return;
    }

    // ---- the document --------------------------------------------------
    // Everything below is the reader's key mode, bare letters included.
    if (matchCombo(e, keymap.palette)) { e.preventDefault(); openPalette(); return; }
    if (matchCombo(e, keymap.settings)) { e.preventDefault(); openSettings(); return; }
    if (matchCombo(e, keymap.toggle_pane)) { e.preventDefault(); togglePane(); return; }

    // The pop-out's own keys, and they exist only while it has focus and is
    // read-only — with the composer up the guard above has already returned,
    // because the target is a textarea.
    //
    // `i` is not in the keymap and deliberately not rebindable: it is not a
    // dreamd action, it is the card's insert key, and it means nothing anywhere
    // else. Claimed above every binding below so that a reader who has bound
    // `i` to something in the document still gets a composer out of the card
    // they are looking at.
    //
    // `j`/`k` and the arrows scroll the log, which is what makes focusing the
    // card on open cost nothing: without them a focused card is a keyboard dead
    // end with two exits, and the reader's scroll keys would go nowhere until
    // they pressed Escape. Native arrow scrolling does not do this — it moves
    // the nearest scrollable ancestor of the focused element, and `#agent-log`
    // is a *descendant* of the card that holds focus.
    if (popoutOpen() && !popout.editing && popoutHasFocus()) {
      if (matchCombo(e, "i")) { e.preventDefault(); setEditing(true); return; }
      const step = e.key === "PageDown" ? 1 : e.key === "PageUp" ? -1 : 0;
      const line = matchCombo(e, "j") || e.key === "ArrowDown" ? 1
        : matchCombo(e, "k") || e.key === "ArrowUp" ? -1 : 0;
      if (step || line) {
        e.preventDefault();
        const log = $("agent-log");
        log.scrollTop += step ? step * log.clientHeight * 0.9 : line * 64;
        return;
      }
    }

    // The configured highlight key turns the current selection into a dreamd
    // highlight and prompts for an annotation. It does NOT toggle mode. Bare
    // `h` is the pre-keymap shortcut, kept as an alias unless turned off.
    if ((keymap.quick_highlight && matchCombo(e, "h")) || matchCombo(e, keymap.highlight)) {
      e.preventDefault();
      triggerHighlight();
      return;
    }
    if (matchCombo(e, keymap.toggle_view)) { e.preventDefault(); toggleView(); return; }
    if (matchCombo(e, keymap.toggle_tree)) { e.preventDefault(); toggleTree(); return; }
    if (matchCombo(e, keymap.toggle_outline)) { e.preventDefault(); toggleOutline(); return; }
    if (matchCombo(e, keymap.toggle_stack)) { e.preventDefault(); toggleStack(); return; }
    if (matchCombo(e, keymap.jump_top)) { e.preventDefault(); jumpTop(); return; }
    if (matchCombo(e, keymap.jump_bottom)) { e.preventDefault(); jumpBottom(); return; }
    // The scroll motions, checked before pane navigation so that in `vim` mode —
    // where `pane_right` strips to a bare `j` — the key that arrives keeps
    // scrolling. Losing a pane key to a rebind is a nuisance; losing `j` would
    // be losing the reason to be in vim mode.
    if (matchCombo(e, keymap.scroll_down)) { e.preventDefault(); scrollLine(1); return; }
    if (matchCombo(e, keymap.scroll_up)) { e.preventDefault(); scrollLine(-1); return; }
    if (matchCombo(e, keymap.scroll_half_down)) { e.preventDefault(); scrollHalf(1); return; }
    if (matchCombo(e, keymap.scroll_half_up)) { e.preventDefault(); scrollHalf(-1); return; }
    // Pane navigation in the reader's own spelling. Below the scroll keys, not
    // above them, because that is the only ordering `vim` mode survives.
    if (matchCombo(e, keymap.pane_left)) { e.preventDefault(); focusPane(-1); return; }
    if (matchCombo(e, keymap.pane_right)) { e.preventDefault(); focusPane(1); return; }
    if (matchCombo(e, keymap.next_file)) { e.preventDefault(); stepFile(1); return; }
    if (matchCombo(e, keymap.prev_file)) { e.preventDefault(); stepFile(-1); return; }
    if (matchCombo(e, keymap.find)) { e.preventDefault(); openFind(); return; }
    if (matchCombo(e, keymap.find_next)) { e.preventDefault(); stepFind(1); return; }
    if (matchCombo(e, keymap.find_prev)) { e.preventDefault(); stepFind(-1); return; }
    if (matchCombo(e, keymap.set_mark)) { e.preventDefault(); setMark(); return; }
    if (matchCombo(e, keymap.jump_mark)) { e.preventDefault(); jumpMark(); return; }
    // `Ctrl+[` and `Ctrl+]` deliberately echo bare `[` / `]` above: those step
    // through the tree's order, these step through where you have actually
    // been. Same hand shape, one modifier apart.
    if (matchCombo(e, keymap.jump_back)) { e.preventDefault(); jumpHistory(-1); return; }
    if (matchCombo(e, keymap.jump_forward)) { e.preventDefault(); jumpHistory(1); return; }
    if (matchCombo(e, keymap.send_stack)) { e.preventDefault(); runStack([]); return; }
    // The hidden tmux binding (D6). `Option<String>` on the Rust side, so it is
    // absent from the keymap JSON entirely unless somebody set it by hand — the
    // guard is what stops `matchCombo(e, undefined)` from being asked.
    if (keymap.send_stack_tmux && matchCombo(e, keymap.send_stack_tmux)) {
      e.preventDefault();
      sendStackTmux([]);
      return;
    }
    if (matchCombo(e, keymap.copy_stack)) {
      // Don't hijack a real copy: if text is selected, let the OS copy it.
      const hasSelection = window.getSelection && window.getSelection().toString().trim();
      if (hasSelection) return;
      e.preventDefault();
      copyStack();
      return;
    }
  });
}

// ---- tooltips ------------------------------------------------------------
// Icon-only buttons carry `data-tip` (label) and optionally `data-tip-key`
// (a keymap field name). Native `title` is deliberately unused so the popup
// appears instantly and can render the keybind alongside the label.
let tipTimer = null;
let tipTarget = null;

function wireTooltips() {
  document.addEventListener("mouseover", (e) => {
    const el = e.target.closest && e.target.closest("[data-tip]");
    if (el !== tipTarget) el ? scheduleTip(el) : hideTip();
  });
  document.addEventListener("mouseout", (e) => {
    const el = e.target.closest && e.target.closest("[data-tip]");
    if (el && el === tipTarget && !el.contains(e.relatedTarget)) hideTip();
  });
  // Keyboard focus gets the same affordance.
  document.addEventListener("focusin", (e) => {
    const el = e.target.closest && e.target.closest("[data-tip]");
    if (el) showTip(el); else hideTip();
  });
  document.addEventListener("focusout", hideTip);
  // A click means the user knows what the button does; get out of the way.
  document.addEventListener("mousedown", hideTip, true);
  // Capture-phase, so this fires for every scroll of the reading pane. Guarded
  // and passive so scrolling a document with no tooltip showing does no work
  // and never blocks the compositor.
  window.addEventListener(
    "scroll",
    () => { if (tipTarget) hideTip(); },
    { capture: true, passive: true }
  );
}

function scheduleTip(el) {
  hideTip();
  tipTarget = el; // claimed up front so mousemove inside the button won't restart the timer
  tipTimer = setTimeout(() => showTip(el), 350);
}

function showTip(el) {
  clearTimeout(tipTimer);
  tipTarget = el;
  const tip = $("tooltip");
  // Through `displayCombo`, so a tooltip shows the key this mode actually
  // wants pressed rather than the canonical form the config file stores.
  const combo = el.dataset.tipKey ? keymap[el.dataset.tipKey] : null;
  tip.innerHTML = escapeHtml(el.dataset.tip) +
    (combo ? `<span class="tt-key">${escapeHtml(displayCombo(combo))}</span>` : "");
  tip.classList.add("show");

  // Prefer below the button; flip above when it would clip the viewport.
  const r = el.getBoundingClientRect();
  const t = tip.getBoundingClientRect();
  const gap = 6;
  let top = r.bottom + gap;
  if (top + t.height > window.innerHeight - 4) top = r.top - t.height - gap;
  let left = r.left + r.width / 2 - t.width / 2;
  left = Math.max(4, Math.min(left, window.innerWidth - t.width - 4));
  tip.style.top = `${Math.max(4, top)}px`;
  tip.style.left = `${left}px`;
}

function hideTip() {
  clearTimeout(tipTimer);
  tipTarget = null;
  $("tooltip").classList.remove("show");
}

async function copyStack() {
  const pairs = await invoke("get_stack");
  if (!pairs.length) { toast("Stack is empty"); return; }
  try {
    const text = await invoke("stack_query_text");
    await invoke("copy_to_clipboard", { text });
    toast(`Copied ${pairs.length} pair${pairs.length > 1 ? "s" : ""} to clipboard`);
  } catch (e) { toast(String(e)); }
}

/// Rewrite a stored combo into the form the reader's `keymap.mode` expects them
/// to press. The twin of `KeyMode::resolve` in `src-tauri/src/config.rs` —
/// change one, change the other; the tests there pin the semantics.
///
/// Every consumer of a combo goes through here — `matchCombo`, `displayCombo`
/// and `comboClashes` — which is what makes a mode a *rendering* of the keymap
/// rather than a second keymap: the config file keeps one canonical `Ctrl+…`
/// form, rebinding an action changes it in all three modes, and switching mode
/// rewrites nothing on disk.
///
/// Only `Ctrl` (and `Meta`, so a combo recorded in `mac` mode is not stranded)
/// moves. A binding that is already bare stays bare in all three modes: modes
/// respell modifiers, they never add one. That is the whole reason `linux` can
/// be the default and still be byte-for-byte the old behaviour.
function resolveCombo(combo) {
  return resolveComboIn(combo, keyMode());
}

function keyMode() {
  return (keymap && keymap.mode) || "linux";
}

/// The mode that applies while a text field has focus — the twin of
/// `KeyMode::in_field`.
///
/// `vim`'s premise is that the document is not a text field, so the letters are
/// free. Inside one that premise is false: a bare `f` is an `f` being typed,
/// and a binding claiming it makes the field unusable. So vim gives the
/// modifier back and `mac` does not have to, a modified binding being safe to
/// claim from a field either way. This is what keeps the palette's next and
/// previous on Ctrl+N and Ctrl+P however the rest of the map reads.
function fieldCombo(combo) {
  return resolveComboIn(combo, keyMode() === "vim" ? "linux" : keyMode());
}

function resolveComboIn(combo, mode) {
  if (!combo) return combo;
  if (mode === "linux") return combo;
  const parts = combo.split("+");
  // Pop the key first: `Ctrl++` is the literal plus key and splits to a
  // trailing empty segment that is the key, not an empty modifier.
  const key = parts.pop();
  const out = [];
  for (const p of parts) {
    const primary = /^(ctrl|meta)$/i.test(p);
    if (!primary) out.push(p);              // unknown modifiers pass through
    else if (mode === "mac") out.push("Meta");
    // vim: dropped
  }
  out.push(key);
  return out.join("+");
}

/// `matchCombo` for a handler that runs while a text field has focus: the
/// binding is matched in its *field* spelling, which always keeps a modifier.
///
/// Every keydown handler attached to an input or textarea uses this, and so
/// does the branch of the global handler that runs before a focused field takes
/// over. A field that could be typed into and commanded with the same keypress
/// is a field with no way to type those letters.
function matchField(e, combo) {
  return matchResolved(e, fieldCombo(combo));
}

/// The identity of a binding: two combos with the same key are the same
/// shortcut, whatever their spelling. `matchCombo` compares case-insensitively
/// and by modifier *set*, so anything asking "are these the same binding?" has
/// to do the same or it will miss real collisions — `Ctrl+M` resolves to `M`
/// while `set_mark` is `m`, and those are one key, not two.
///
/// Modifiers are sorted as well as lowercased. `comboFromEvent` always writes
/// them in one order, but a hand-edited `config.toml` need not.
/// Takes a combo already in its pressed spelling — callers pick that with
/// `actionCombo`, because a field binding and a document one are spelled
/// differently in `vim` mode and comparing the stored form would miss both.
function comboKey(spelled) {
  if (!spelled) return "";
  const parts = spelled.toLowerCase().split("+");
  const key = parts.pop();
  return [...parts.sort(), key].join("+");
}

/// The spelling an action is actually pressed in: a `field` action never loses
/// its modifier, everything else follows the reader's key mode.
function actionCombo(action, combo) {
  return action.field ? fieldCombo(combo) : resolveCombo(combo);
}

function matchCombo(e, combo) {
  return matchResolved(e, resolveCombo(combo));
}

/// The comparison itself, against a combo already in the spelling to be
/// pressed. `matchCombo` and `matchField` differ only in which spelling they
/// hand it.
function matchResolved(e, combo) {
  if (!combo) return false;
  const parts = combo.toLowerCase().split("+");
  const key = parts.pop();
  if (e.ctrlKey !== parts.includes("ctrl")) return false;
  if (e.shiftKey !== parts.includes("shift")) return false;
  if (e.altKey !== parts.includes("alt")) return false;
  if (e.metaKey !== parts.includes("meta")) return false;
  return (e.key || "").toLowerCase() === key;
}

/// Render a combo the way the reader will press it: through `resolveCombo`
/// first, so a tooltip in `vim` mode says `f` and not `Ctrl+F`, then through the
/// platform's symbols. Purely cosmetic — what gets stored is always the
/// canonical `Ctrl+Shift+X` form.
const MAC_SYMBOLS = { ctrl: "⌃", shift: "⇧", alt: "⌥", meta: "⌘" };
function displayCombo(combo) {
  return displayResolved(resolveCombo(combo));
}

/// How a field binding reads — the counterpart of `matchField`, so a label
/// beside one names the key that actually works there.
function displayField(combo) {
  return displayResolved(fieldCombo(combo));
}

function displayResolved(resolved) {
  if (!resolved) return "—";
  if (!document.body.classList.contains("mac")) return resolved;
  const parts = resolved.split("+");
  const key = parts.pop();
  return parts.map((p) => MAC_SYMBOLS[p.toLowerCase()] || p + "+").join("") + key;
}

// ---- settings panel ------------------------------------------------------
// Three tabs over one payload from `get_settings`. Everything the panel writes
// goes through `set_config`, which is the same path `dreamd config set` takes,
// so a change made here and one made from the shell produce the same file.

const KEY_ACTIONS = [
  { id: "palette", label: "Open file palette", sub: "Fuzzy find markdown files" },
  // `field: true` — pressed while a text field has focus, so these keep their
  // modifier in every key mode (see `fieldCombo`). The panel must render and
  // clash-check them in that spelling or it names a key that does nothing.
  { id: "palette_next", label: "Palette: next result", field: true, sub: "From inside the palette's query box, so it keeps its modifier even in vim mode" },
  { id: "palette_prev", label: "Palette: previous result", field: true },
  { id: "highlight", label: "Highlight selection", sub: "Turn the selection into evidence and ask for an annotation" },
  { id: "save_annotation", label: "Save annotation", field: true, sub: "From inside the annotation box, so it keeps its modifier even in vim mode" },
  { id: "toggle_outline", label: "Toggle contents panel", sub: "Outline of the open document's headings" },
  { id: "toggle_tree", label: "Toggle file tree", sub: "Collapse or restore the sidebar" },
  { id: "toggle_view", label: "Toggle view mode", sub: "Hide the titlebar, sidebar and panels — Esc also exits" },
  { id: "toggle_stack", label: "Toggle stack panel" },
  { id: "toggle_pane", label: "Toggle Claude Code pane", sub: "Claude Code in this repo, docked beside the document — the same key gets you back out of it" },
  { id: "jump_top", label: "Jump to top", sub: "Scroll the open document to the start" },
  { id: "jump_bottom", label: "Jump to bottom" },
  { id: "scroll_down", label: "Scroll down a line", sub: "Bare in every key mode — scrolling should never cost a modifier" },
  { id: "scroll_up", label: "Scroll up a line" },
  { id: "scroll_half_down", label: "Scroll down half a screen", sub: "vim's Ctrl+D, without the Ctrl" },
  { id: "scroll_half_up", label: "Scroll up half a screen" },
  { id: "pane_left", label: "Focus pane to the left", sub: "Walks the visible panes: sidebar, document, contents, stack, agent" },
  { id: "pane_right", label: "Focus pane to the right" },
  { id: "next_file", label: "Next file", sub: "Move through the sidebar's order without touching the tree — wraps at the ends" },
  { id: "prev_file", label: "Previous file" },
  { id: "find", label: "Find in document", sub: "Search the open file's text; Enter keeps the matches and closes the bar, Esc clears them" },
  { id: "find_next", label: "Next match", sub: "Works with the bar closed, like vim's n" },
  { id: "find_prev", label: "Previous match" },
  { id: "set_mark", label: "Set mark", sub: "Remembers this file and scroll position until the app quits — one mark, replaced each time" },
  { id: "jump_mark", label: "Jump to mark", sub: "Back to the mark, across files" },
  { id: "jump_back", label: "Jump back", sub: "The position before the last link, tree, palette or mark jump" },
  { id: "jump_forward", label: "Jump forward", sub: "Undo a jump back — cleared by any new jump" },
  { id: "send_stack", label: "Send stack to agent" },
  { id: "copy_stack", label: "Copy stack", sub: "Ignored while text is selected, so OS copy still works" },
  { id: "settings", label: "Open settings" },
];

let settings = null;        // the last `get_settings` payload
let recording = null;       // { action, btn } while capturing a keybind
let draft = null;           // { css } being edited in the Custom tab
const themeCache = new Map(); // name -> full stylesheet
let draftTimer = null;

function wireSettings() {
  $("btn-settings").onclick = openSettings;
  $("settings-close").onclick = closeSettings;
  $("settings-done").onclick = closeSettings;
  for (const tab of document.querySelectorAll(".st-tab")) {
    tab.onclick = () => selectPane(tab.dataset.pane);
  }
  $("st-save-theme").onclick = saveCustomTheme;
  $("st-custom-css").oninput = () => {
    draft = { css: $("st-custom-css").value };
    clearTimeout(draftTimer);
    draftTimer = setTimeout(() => { previewCss(draft.css); renderVars(); }, 180);
  };
}

async function openSettings() {
  const overlay = $("settings-overlay");
  if (overlay.classList.contains("open")) return;
  try {
    settings = await invoke("get_settings");
  } catch (e) { toast(String(e)); return; }
  $("settings-path").textContent = settings.config_path;
  renderKeys();
  overlay.classList.add("open");
  selectPane("keys");
}

function closeSettings() {
  cancelRecording();
  previewCss("");
  $("settings-overlay").classList.remove("open");
}

function selectPane(pane) {
  for (const t of document.querySelectorAll(".st-tab")) t.classList.toggle("sel", t.dataset.pane === pane);
  for (const p of document.querySelectorAll(".st-pane")) p.classList.toggle("sel", p.id === "st-pane-" + pane);
  if (pane === "window") renderWindow();
  if (pane === "themes") renderThemes();
  if (pane === "custom") openCustom();
}

/// Refetch after any write, so the panel shows what the *merged* config says
/// rather than what it just asked for — a repo-local `.dreamd.toml` can shadow
/// a key we saved globally, and the badge is only honest if we re-read.
async function applyPatch(patch) {
  try {
    settings = await invoke("set_config", { patch });
    keymap = settings.config.keymap;
    return true;
  } catch (e) { toast(String(e)); return false; }
}

function shadowed(key) {
  return settings && settings.local_overrides.includes(key);
}

// ---- keys tab ----
const KEY_MODES = [
  { id: "linux", label: "Ctrl", sub: "Ctrl+F opens the palette. The default." },
  { id: "mac", label: "Cmd", sub: "Cmd+F instead — including Cmd+C and Cmd+F, which dreamd then claims from the webview" },
  { id: "vim", label: "None", sub: "Bare f. Every binding loses its Ctrl; Shift and Alt survive" },
];

function renderKeys() {
  const box = $("st-keys");
  box.innerHTML = "";
  const clashes = comboClashes();

  // The mode picker, above the list it re-renders. It changes how every combo
  // below is *spelled*, never what is stored — which is why switching it and
  // switching back is a no-op on the config file.
  const modeRow = document.createElement("div");
  modeRow.className = "st-row";
  const current = keymap.mode || "linux";
  modeRow.innerHTML =
    `<span class="lbl">Modifier for every shortcut` +
    `<span class="sub">${escapeHtml(KEY_MODES.find((m) => m.id === current).sub)}</span>` +
    (shadowed("keymap.mode") ? `<span class="sub shadowed">overridden by .dreamd.toml in this repo</span>` : "") +
    `</span>`;
  const sel = document.createElement("select");
  for (const m of KEY_MODES) {
    const opt = document.createElement("option");
    opt.value = m.id;
    opt.textContent = m.label;
    opt.selected = m.id === current;
    sel.appendChild(opt);
  }
  sel.onchange = async () => {
    if (await applyPatch({ keymap: { mode: sel.value } })) renderKeys();
    else sel.value = current;
  };
  modeRow.appendChild(sel);
  box.appendChild(modeRow);

  for (const action of KEY_ACTIONS) {
    const row = document.createElement("div");
    row.className = "st-row";
    // Names the row for anything selecting one. The mode picker above and the
    // quick-highlight checkbox below are `.st-row` too and carry no combo, so
    // position in the list is not an identity — `ui-check` asked for the first
    // row's `button.combo` and got the picker's, which has none.
    row.dataset.action = action.id;
    const combo = keymap[action.id];
    row.innerHTML =
      `<span class="lbl">${escapeHtml(action.label)}` +
      (action.sub ? `<span class="sub">${escapeHtml(action.sub)}</span>` : "") +
      (shadowed("keymap." + action.id) ? `<span class="sub shadowed">overridden by .dreamd.toml in this repo</span>` : "") +
      `</span>`;
    const btn = document.createElement("button");
    btn.className = "combo" + (combo && clashes.has(comboKey(actionCombo(action, combo))) ? " clash" : "");
    btn.textContent = action.field ? displayField(combo) : displayCombo(combo);
    // The stored form, which in a non-`linux` mode is not what the button says.
    btn.title = combo || "";
    btn.onclick = () => startRecording(action.id, btn);
    row.appendChild(btn);
    box.appendChild(row);
  }

  const quick = document.createElement("div");
  quick.className = "st-row";
  quick.innerHTML =
    `<span class="lbl">Also accept a bare <code>h</code> to highlight` +
    `<span class="sub">How the app worked before keybinds were configurable</span></span>`;
  const cb = document.createElement("input");
  cb.type = "checkbox";
  cb.checked = !!keymap.quick_highlight;
  cb.onchange = async () => {
    if (!(await applyPatch({ keymap: { quick_highlight: cb.checked } }))) cb.checked = !cb.checked;
  };
  quick.appendChild(cb);
  box.appendChild(quick);

  const reset = document.createElement("button");
  reset.textContent = "Reset all shortcuts";
  reset.style.marginTop = "14px";
  reset.onclick = async () => {
    const defaults = await invoke("default_keymap");
    if (await applyPatch({ keymap: defaults })) { renderKeys(); toast("Shortcuts reset"); }
  };
  box.appendChild(reset);

  if (clashes.size) {
    const warn = document.createElement("p");
    warn.className = "st-warn";
    // Named differently in `vim` mode, because there the clash is not something
    // the reader did — it is what dropping the modifier off every binding costs,
    // and saying "two actions share a shortcut" would read as their mistake.
    warn.textContent = (keymap.mode === "vim")
      ? "Without a modifier some bindings collide — the first one listed wins. " +
        "Rebind the loser, or switch the modifier above."
      : "Two actions share a shortcut — the first one listed wins.";
    box.appendChild(warn);
  }
}

/// Combos bound to more than one action. The global handler is an if-chain, so
/// a duplicate is not an error — it just means the later action is unreachable.
///
/// Compared *after* `resolveCombo`, which is what makes this carry its weight in
/// `vim` mode: stripping the primary modifier off every binding collapses pairs
/// that were distinct with it on — `Ctrl+M` onto `m`, `Ctrl+N` onto `n`,
/// `Ctrl+[` onto `[`, the new `Ctrl+J` onto `j` — and those collisions are
/// exactly the ones a reader has no other way to discover. Keyed by the resolved
/// combo, so the caller must resolve before asking.
function comboClashes() {
  const seen = new Map();
  const dupes = new Set();
  for (const a of KEY_ACTIONS) {
    const c = keymap[a.id];
    if (!c) continue;
    const k = comboKey(actionCombo(a, c));
    if (seen.has(k)) dupes.add(k);
    seen.set(k, a.id);
  }
  // Bare `h` is a binding too, and in `vim` mode it is the one that shadows
  // pane-left. It is not in KEY_ACTIONS because it is a checkbox, not a combo.
  if (keymap.quick_highlight) {
    for (const a of KEY_ACTIONS) {
      if (keymap[a.id] && comboKey(actionCombo(a, keymap[a.id])) === "h") dupes.add("h");
    }
  }
  return dupes;
}

function startRecording(action, btn) {
  cancelRecording();
  recording = { action, btn, listener: onRecordKey };
  btn.classList.add("rec");
  btn.textContent = "Press keys…";
  document.addEventListener("keydown", onRecordKey, true);
}

/// Returns true if it actually cancelled something, so Escape can be consumed
/// by the recorder instead of closing the whole panel.
function cancelRecording() {
  if (!recording) return false;
  document.removeEventListener("keydown", recording.listener, true);
  recording.btn.classList.remove("rec");
  recording = null;
  renderKeys();
  return true;
}

async function onRecordKey(e) {
  e.preventDefault();
  e.stopPropagation();
  if (e.key === "Escape") { cancelRecording(); return; }
  const combo = comboFromEvent(e);
  if (!combo) return; // a modifier on its own, or a key we can't encode
  const { action } = recording;
  cancelRecording();
  if (await applyPatch({ keymap: { [action]: combo } })) {
    renderKeys();
    toast(`${action} → ${displayCombo(combo)}`);
  }
}

const MODIFIER_KEYS = ["Control", "Shift", "Alt", "Meta"];

/// Encode a keypress as a combo to *store*, which is not the same as the combo
/// the reader just pressed — the config file speaks one canonical `Ctrl+…`
/// dialect and `resolveCombo` translates it back out per mode.
///
/// The only translation needed on the way in is `mac`: there Cmd *is* the
/// primary modifier, so a recorded Cmd is written down as `Ctrl` and the
/// binding keeps working after a switch to `linux`. Two consequences worth
/// naming, both inherent to the mode rather than to this function: in `mac`
/// mode a real Ctrl records as the primary modifier too (that mode has no way
/// to spell Ctrl separately), and a key recorded in `vim` mode is stored bare —
/// which is exactly what was pressed, and so stays bare in every other mode.
function comboFromEvent(e) {
  const key = e.key || "";
  if (MODIFIER_KEYS.includes(key)) return null;
  // `matchCombo` splits on "+", so a combo whose key is "+" can never match.
  if (key === "+") { toast("“+” can't be used as a shortcut key"); return null; }
  const macMode = (keymap.mode || "linux") === "mac";
  const parts = [];
  if (e.ctrlKey || (macMode && e.metaKey)) parts.push("Ctrl");
  if (e.shiftKey) parts.push("Shift");
  if (e.altKey) parts.push("Alt");
  if (e.metaKey && !macMode) parts.push("Meta");
  parts.push(key);
  return parts.join("+");
}

// ---- window tab ----
//
// The two bars the platform draws around the reader, rather than anything dreamd
// paints: the native menubar and the window manager's titlebar. Off by default
// everywhere except the macOS titlebar, which is an overlay and costs no room —
// and this pane is the only way back once they are gone, which is why it is a
// tab of its own and not a footnote in Themes.
//
// The Rust side applies both to the live window inside `set_config`, so there is
// nothing to re-render here beyond the checkbox itself and no restart to
// prompt for.
const WINDOW_TOGGLES = [
  {
    key: "menubar",
    label: "Native menubar",
    sub: "The File / Edit / Help bar. Its two Open shortcuts go with it — click the repo name above the file tree to move to another folder.",
    // On macOS the menubar is the application's, not this window's, and
    // `hide_menu` is a documented no-op there — the row would be a switch
    // wired to nothing.
    mac: false,
  },
  {
    key: "titlebar",
    label: "Native titlebar",
    sub: "The window manager's bar, with close, minimize and maximize. Drag the top edge of the window to move it without one.",
    mac: true,
  },
];

function renderWindow() {
  const box = $("st-window");
  const isMac = document.body.classList.contains("mac");
  box.innerHTML = "";

  box.appendChild(sectionHeader("Window chrome"));
  for (const t of WINDOW_TOGGLES) {
    if (isMac && !t.mac) continue;
    const row = document.createElement("div");
    row.className = "st-row";
    row.innerHTML =
      `<span class="lbl">${escapeHtml(t.label)}` +
      `<span class="sub">${escapeHtml(t.sub)}</span></span>`;
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = !!settings.config.ui[t.key];
    cb.onchange = async () => {
      if (!(await applyPatch({ ui: { [t.key]: cb.checked } }))) cb.checked = !cb.checked;
    };
    row.appendChild(cb);
    box.appendChild(row);
  }

  // The agent surface. A checkbox rather than a two-value select because there
  // is a supported answer and a fallback, not a preference between equals — and
  // it is phrased as the fallback ("use the terminal instead") so that leaving
  // it alone is the native surface, which is what it is.
  //
  // It takes effect on the pane's *next* open: `agent_prefs` is read once, and
  // swapping bodies under a live conversation would throw away the one the
  // reader is reading.
  box.appendChild(sectionHeader("Agent pane"));
  const row = document.createElement("div");
  row.className = "st-row";
  row.innerHTML =
    `<span class="lbl">Terminal agent pane` +
    `<span class="sub">Run Claude Code's own terminal interface instead of dreamd's. ` +
    `A fallback for anything the native pane cannot draw yet; takes effect the next time the pane opens.</span></span>`;
  const term = document.createElement("input");
  term.type = "checkbox";
  term.checked = (settings.config.agent?.surface ?? "native") === "terminal";
  term.onchange = async () => {
    const surface = term.checked ? "terminal" : "native";
    if (!(await applyPatch({ agent: { surface } }))) term.checked = !term.checked;
    // So the next open reads the new value rather than the cached one.
    else if (pty.prefs) pty.prefs.surface = surface;
    // The pop-out belongs to the native surface, and the row below says so by
    // going dead rather than by disappearing.
    pop.disabled = term.checked;
  };
  row.appendChild(term);
  box.appendChild(row);

  // Where the conversation is drawn. A select rather than two checkboxes
  // because the three answers are exclusive and the middle one is the whole
  // point of the setting: a stack send produces something to read, and a dock
  // charges the document its width for as long as it is open.
  //
  // Below the surface toggle, and dimmed by it: the pop-out is native-only, and
  // a control that silently does nothing is worse than one that says why.
  const popRow = document.createElement("div");
  popRow.className = "st-row";
  popRow.innerHTML =
    `<span class="lbl">Pop-out agent` +
    `<span class="sub">A card centred on the window instead of a dock, read-only until you click it or press ` +
    `<b>i</b>. Escape gives the composer back, then puts the card away. ` +
    `Takes effect the next time the pane opens.</span></span>`;
  const pop = document.createElement("select");
  for (const [value, label] of [
    ["never", "Never — dock it"],
    ["send", "When I send the stack"],
    ["always", "Always"],
  ]) {
    const opt = document.createElement("option");
    opt.value = value;
    opt.textContent = label;
    pop.appendChild(opt);
  }
  pop.value = settings.config.agent?.popout ?? "never";
  // Disabled rather than hidden when the terminal fallback is on: hiding it
  // would leave a reader who turned the terminal on wondering where the setting
  // went, and the sentence above already says which surface it belongs to.
  pop.disabled = term.checked;
  pop.onchange = async () => {
    const before = pty.prefs?.popout ?? "never";
    if (!(await applyPatch({ agent: { popout: pop.value } }))) {
      pop.value = before;
      return;
    }
    if (pty.prefs) pty.prefs.popout = pop.value;
  };
  popRow.appendChild(pop);
  box.appendChild(popRow);

  // Print, which is an action rather than a preference and the only one in this
  // panel. It lived in the titlebar and moved here because it is rare and
  // deliberate — a keybind was never worth spending on it (`Ctrl+P` is
  // `palette_prev`), and a permanent slot on the bar was the same bet in
  // pixels. This tab is where it does least harm: the panel is already the
  // place you go for the things you do once.
  box.appendChild(sectionHeader("Document"));
  const printRow = document.createElement("div");
  printRow.className = "st-row";
  printRow.innerHTML =
    `<span class="lbl">Print or save as PDF` +
    `<span class="sub">Opens your system print dialog for the open document. ` +
    `Choose “Save as PDF” there to export — dreamd picks no path and writes no file.</span></span>`;
  const printBtn = document.createElement("button");
  printBtn.textContent = "Print…";
  // Settings gets out of the way first: the dialog is the OS's and belongs over
  // the document it is about to print, not over the panel that asked for it.
  // (The `@media print` sheet hides `.modal-overlay` regardless, so this is
  // about what the reader is left looking at, not about what comes out.)
  //
  // Guarded on there being a document, which is `printDocument`'s own refusal
  // read a second time rather than trusted: closing the panel to deliver a
  // "Nothing open to print" toast is the wrong half of the gesture.
  printBtn.onclick = () => {
    if (currentFile) closeSettings();
    printDocument();
  };
  printRow.appendChild(printBtn);
  box.appendChild(printRow);
}

/// An uppercase divider, the same one the Themes and Custom tabs write inline.
function sectionHeader(label) {
  const el = document.createElement("div");
  el.className = "st-sect";
  el.textContent = label;
  return el;
}

// ---- themes tab ----
async function cacheTheme(name) {
  if (themeCache.has(name)) return themeCache.get(name);
  try {
    const css = await invoke("theme_css", { name });
    themeCache.set(name, css);
    return css;
  } catch { return null; }
}

/// Light / Dark / System, independent of which family is selected.
async function setMode(mode) {
  if (!(await applyPatch({ mode }))) return;
  modePref = mode;
  await loadTheme();
  renderMode();
  renderThemes();
  toast(shadowed("mode")
    ? `Saved, but .dreamd.toml pins ${settings.config.mode} in this repo`
    : `Appearance: ${mode}`);
}

function renderMode() {
  const row = $("st-mode");
  if (!row) return;
  row.innerHTML = "";
  const current = settings.config.mode || "system";
  for (const mode of ["light", "dark", "system"]) {
    const b = document.createElement("button");
    // "System" alone doesn't say what you are looking at.
    b.textContent = mode === "system" ? `System (${settings.scheme})` : mode;
    b.className = "st-mode-btn" + (mode === current ? " sel" : "");
    b.onclick = () => setMode(mode);
    row.appendChild(b);
  }
}

async function renderThemes() {
  const grid = $("st-theme-grid");
  await Promise.all(settings.themes.map((t) => cacheTheme(t.name)));
  renderMode();
  grid.innerHTML = "";

  for (const info of settings.themes) {
    const css = themeCache.get(info.name) || "";
    const card = document.createElement("div");
    card.className = "th-card" + (info.name === settings.theme ? " sel" : "");
    card.innerHTML =
      `<div class="top"><span class="name">${escapeHtml(info.name)}</span>` +
      `<span class="kind">${escapeHtml(info.kind)}</span></div>` +
      `<div class="th-swatch"></div>`;
    // Set as a property, not an interpolated `style="…"`: the value comes from
    // a CSS file and would otherwise carry arbitrary declarations into the
    // attribute.
    const swatch = card.querySelector(".th-swatch");
    // Read in the appearance that is actually on screen, so the swatch answers
    // "what will this look like" rather than "what does the last block in the
    // file say" — which, for a family, is always the dark one.
    for (const v of ["--bg", "--text", "--accent", "--hl"]) {
      const stripe = document.createElement("i");
      stripe.style.background = readCssVar(css, v, appearance) || "transparent";
      swatch.appendChild(stripe);
    }

    card.onclick = () => {
      previewCss(css);
      for (const c of grid.children) c.classList.remove("sel");
      card.classList.add("sel");
    };

    const acts = document.createElement("div");
    acts.className = "acts";
    acts.appendChild(button("Apply", "primary", async (ev) => {
      ev.stopPropagation();
      if (await applyPatch({ theme: info.name })) {
        previewCss("");
        await loadTheme();
        renderThemes();
        toast(shadowed("theme")
          ? `Saved, but .dreamd.toml pins ${settings.theme} in this repo`
          : `Theme: ${info.name}`);
      }
    }));
    acts.appendChild(button("Duplicate", "", async (ev) => {
      ev.stopPropagation();
      // The palette file itself, not the first `:root` block regexed back out
      // of base+palette — a copy has to carry both appearances to stay a
      // family.
      draft = { css: await paletteOf(info.name), editing: appearance };
      $("st-save-name").value = info.name + "-copy";
      selectPane("custom");
    }));
    if (info.kind === "user") {
      acts.appendChild(button("Delete", "danger", async (ev) => {
        ev.stopPropagation();
        try { await invoke("delete_theme", { name: info.name }); } catch (e) { toast(String(e)); return; }
        themeCache.delete(info.name);
        settings = await invoke("get_settings");
        await loadTheme();
        renderThemes();
      }));
    }
    card.appendChild(acts);
    grid.appendChild(card);
  }
}

function button(label, cls, onclick) {
  const b = document.createElement("button");
  b.textContent = label;
  if (cls) b.className = cls;
  b.onclick = onclick;
  return b;
}

/// Live preview: an extra stylesheet after `#user-theme`, so nothing about the
/// saved theme is disturbed and cancelling is a single assignment.
function previewCss(css) {
  $("theme-preview").textContent = css || "";
}

// ---- custom theme tab ----
async function openCustom() {
  if (!draft) {
    draft = { css: await paletteOf(settings.theme), editing: appearance };
    if (!$("st-save-name").value) $("st-save-name").value = (settings.theme || "dreamd") + "-copy";
  }
  $("st-custom-note").textContent =
    `Edits preview live. Saving writes ${settings.themes_dir}/<name>.css and switches to it.`;
  $("st-custom-css").value = draft.css;
  renderBlockPicker();
  renderVars();
  previewCss(draft.css);
}

/// A palette's own file. Asking Rust beats recovering it from `theme_css`: that
/// meant regexing the first `:root` block back out of base+palette, which finds
/// the *shared* block of a family and silently drops both mode blocks — and had
/// to dodge the `:root { --bg: … }` example inside theme.css's header comment.
async function paletteOf(name) {
  if (!name) return ":root {\n}\n";
  try {
    return await invoke("palette_css", { name });
  } catch { return ":root {\n}\n"; }
}

/// Which appearance the var editor is pointed at. Switching re-renders the list
/// and nothing else — the draft text is untouched, so no edit is lost.
function renderBlockPicker() {
  const row = $("st-block");
  if (!row) return;
  row.innerHTML = "";
  if (!blocksOf(draft.css).light && !blocksOf(draft.css).dark) return;
  for (const mode of ["light", "dark"]) {
    const b = document.createElement("button");
    b.textContent = mode;
    b.className = "st-mode-btn" + (mode === draft.editing ? " sel" : "");
    b.onclick = () => { draft.editing = mode; renderBlockPicker(); renderVars(); };
    row.appendChild(b);
  }
}

/// The character ranges of a palette's three blocks — ranges into the string as
/// given, not extracted text, because `setPaletteVar` edits in place and needs
/// real offsets. Comments and strings are skipped where they sit rather than
/// stripped out, which is where this legitimately diverges from the Rust side.
function blocksOf(css) {
  const out = { shared: null, light: null, dark: null };
  let prelude = 0, i = 0;
  while (i < css.length) {
    const c = css[i];
    if (c === "/" && css[i + 1] === "*") {
      const close = css.indexOf("*/", i + 2);
      i = close === -1 ? css.length : close + 2;
      continue;
    }
    if (c === '"' || c === "'") { i = skipString(css, i); continue; }
    if (c === "{") {
      const end = blockEnd(css, i);
      const sel = css.slice(prelude, i);
      const named = modeAttr(sel);
      // `undefined` is "no data-mode"; only a bare `:root` is the shared block.
      const key = named === undefined ? (/(^|[\s,])(:root|html)\s*$/i.test(sel.trim()) ? "shared" : null) : named;
      if (key && !out[key]) out[key] = { start: i + 1, end: end - 1 };
      i = prelude = end;
      continue;
    }
    if (c === "}" || c === ";") { prelude = ++i; continue; }
    i++;
  }
  return out;
}

/// The variables on offer: the shared block plus the appearance being edited.
///
/// Each row carries the *key* of the block it came from, not the range. An edit
/// changes the string's length — `#fff` to `#ffffff`, or an insertion — which
/// invalidates every range after it, so ranges are resolved at write time
/// instead of being cached on the row.
function paletteVars(css, editing) {
  const b = blocksOf(css);
  // Mode block first: a variable declared in both is the mode block's, because
  // that is the one the cascade uses. Attributing it to `shared` would send the
  // edit somewhere it has no visible effect.
  const keys = [editing, "shared"].filter((k) => b[k]);
  // No recognisable blocks: treat the whole file as one. A hand-written palette
  // that puts its variables somewhere unexpected still edits.
  if (!keys.length) keys.push(null);
  const seen = new Set();
  const out = [];
  for (const key of keys) {
    const range = key ? b[key] : { start: 0, end: css.length };
    const body = css.slice(range.start, range.end);
    for (const m of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
      if (seen.has(m[1])) continue;
      seen.add(m[1]);
      out.push({ name: m[1], value: m[2].trim(), block: key });
    }
  }
  return out;
}

/// Targeted replacement rather than regenerating the block, so comments, blank
/// lines and any hand-written rules in the file survive editing. Scoped to one
/// block: with three of them, a document-wide replace edits whichever comes
/// first, which is rarely the one on screen.
function setPaletteVar(css, name, value, block) {
  const r = (block && blocksOf(css)[block]) || { start: 0, end: css.length };
  const body = css.slice(r.start, r.end);
  const re = new RegExp(`(${name}\\s*:\\s*)([^;]*)(;)`);
  if (re.test(body)) {
    return css.slice(0, r.start) + body.replace(re, `$1${value}$3`) + css.slice(r.end);
  }
  // Not declared in this block yet — insert it. Needed to give one appearance
  // its own value for something the family had left shared.
  const insert = `  ${name}: ${value};\n`;
  return css.slice(0, r.end) + insert + css.slice(r.end);
}

const HEX = /^#[0-9a-f]{3,8}$/i;

function renderVars() {
  const box = $("st-vars");
  box.innerHTML = "";
  for (const v of paletteVars(draft.css, draft.editing)) {
    const row = document.createElement("div");
    row.className = "st-var";
    row.innerHTML = `<label>${escapeHtml(v.name)}</label>`;

    if (v.name === "--syntax-theme") {
      const sel = document.createElement("select");
      const current = v.value.replace(/^["']|["']$/g, "");
      for (const name of settings.syntax_themes) {
        const opt = document.createElement("option");
        opt.value = name;
        opt.textContent = name;
        opt.selected = name === current;
        sel.appendChild(opt);
      }
      sel.onchange = () => editVar(v.name, `"${sel.value}"`, false, v.block);
      row.appendChild(sel);
    } else {
      if (HEX.test(v.value)) {
        const picker = document.createElement("input");
        picker.type = "color";
        picker.value = v.value.slice(0, 7);
        picker.oninput = () => editVar(v.name, picker.value, true, v.block);
        row.appendChild(picker);
      }
      const text = document.createElement("input");
      text.type = "text";
      text.value = v.value;
      text.onchange = () => editVar(v.name, text.value, true, v.block);
      row.appendChild(text);
    }
    box.appendChild(row);
  }
}

/// `quiet` skips the re-render of the var list, so typing in a text field
/// doesn't yank focus out from under the user.
function editVar(name, value, quiet, block) {
  draft.css = setPaletteVar(draft.css, name, value, block);
  $("st-custom-css").value = draft.css;
  previewCss(draft.css);
  if (!quiet) renderVars();
}

async function saveCustomTheme() {
  const name = $("st-save-name").value.trim();
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(name)) {
    toast("Name can only use letters, digits, dot, dash and underscore");
    return;
  }
  try {
    await invoke("save_theme", { name, css: draft.css });
  } catch (e) { toast(String(e)); return; }
  themeCache.delete(name);
  if (await applyPatch({ theme: name })) {
    previewCss("");
    await loadTheme();
    toast(`Saved ${name}`);
    selectPane("themes");
  }
}

// ---- utils ---------------------------------------------------------------
const ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" };

// One pass, not four: this runs twice per palette row, 200 rows per keystroke.
function escapeHtml(s) {
  return String(s).replace(/[&<>"]/g, (c) => ESCAPES[c]);
}

let toastTimer = null;
function toast(msg) {
  const t = $("toast");
  t.textContent = msg;
  t.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove("show"), 2600);
}

init();
