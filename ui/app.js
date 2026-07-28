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
let keymap = {
  palette: "Ctrl+F",
  palette_prev: "Ctrl+P",
  palette_next: "Ctrl+N",
  highlight: "Ctrl+H",
  send_stack: "Ctrl+Enter",
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
  next_file: "]",
  prev_file: "[",
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

  // One round trip, two answers: the tree cannot lay itself out at the
  // persisted width without `[ui]`, and asking for it after the keymap would
  // put a second serial IPC in front of the first paint. `get_ui` is a lock
  // read — `get_settings`, which also carries it, walks the themes directory.
  try {
    const [km, ui] = await Promise.all([invoke("get_keymap"), invoke("get_ui")]);
    if (km) keymap = km;
    applyTreeWidth(ui && ui.tree_width);
  } catch (e) {}
  $("search-hint").textContent = `Press ${keymap.palette} to search`;
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

// ---- tree width ----------------------------------------------------------
// The handle on the sidebar's right border and `config.ui.tree_width` are the
// same number. Rust clamps it to this range on the way in, so a stale or
// hand-typed value costs the nearest usable tree rather than a rejected config
// file; these constants mirror `config::TREE_WIDTH_*` and exist so the drag
// never *sends* something out of range in the first place.
const TREE_MIN = 140;
const TREE_MAX = 600;
const TREE_DEFAULT = 260;
let treeWidth = TREE_DEFAULT;

// Written as an inline style on <html>, which outranks any `:root` rule a
// palette might carry.
function applyTreeWidth(px) {
  const want = Number(px) || TREE_DEFAULT;
  treeWidth = Math.round(Math.min(TREE_MAX, Math.max(TREE_MIN, want)));
  document.documentElement.style.setProperty("--tree-width", `${treeWidth}px`);
}

// Debounced: a drag across the window is one config write, not forty.
let treeWidthTimer = null;
function persistTreeWidth() {
  clearTimeout(treeWidthTimer);
  treeWidthTimer = setTimeout(() => {
    invoke("set_config", { patch: { ui: { tree_width: treeWidth } } })
      .catch((e) => console.error(e));
  }, 400);
}

function wireTreeDrag() {
  const handle = $("tree-resize");
  const sidebar = $("sidebar");
  handle.addEventListener("pointerdown", (e) => {
    // No `setPointerCapture`: dragging past the minimum collapses the tree,
    // which hides this element, and a capture held by a `display: none`
    // element is not something to rely on. Window listeners for the length of
    // the drag do the same job and cannot be lost that way.
    e.preventDefault();
    const left = sidebar.getBoundingClientRect().left;
    document.body.classList.add("dragging-tree");
    const onMove = (ev) => {
      const raw = ev.clientX - left;
      // D20: at the extreme the drag *is* `toggle_tree`. The width is left at
      // the last usable one, so expanding again restores what you had rather
      // than snapping back to the default.
      if (raw < TREE_MIN) { document.body.classList.add("nav-collapsed"); return; }
      document.body.classList.remove("nav-collapsed");
      applyTreeWidth(raw);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      document.body.classList.remove("dragging-tree");
      persistTreeWidth();
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
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
  // highlights.
  const doc = active > SCAN_THRESHOLD ? scanTextNodes(contentEl) : null;
  const placements = [];

  for (const h of list) {
    if (h.state === "stale") { addStaleChip(h); continue; }
    const quote = h.quote.trim();
    if (!doc) {
      // Few enough to place as we go; wrapping can only disturb text nodes we
      // have already passed.
      if (!wrapByWalk(contentEl, quote, h.id, h.prior)) addStaleChip(h);
      continue;
    }
    const p = locateInNodes(doc, quote);
    if (p) placements.push({ ...p, id: h.id, prior: h.prior });
    else addStaleChip(h); // active but unlocatable in the DOM
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
}

// Wrap the first occurrence of `quote` that lies within a single text node,
// stopping the walk as soon as it is found. Returns true if it was placed.
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
    (mode === "edit" ? "Save" : "Add to stack") + "  " + displayCombo(keymap.save_annotation);
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
  const m = contentEl.querySelector(`mark.hl[data-id="${id}"]`);
  if (m) unwrap(m);
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

function reconcileStack(list, pairs) {
  // Decided once per refresh, so an enter and an exit in the same pass cannot
  // disagree about whether this render is animated.
  const animate = stackAnimates();

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
    msg.textContent = `No pairs yet. Select text and press ${keymap.highlight}.`;
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
    refreshStack();
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
// at all. Instant, not smooth: everything else that moves this pane
// (`scrollIntoView`, restoring `scrollTop` after a re-render) is instant, and a
// smooth animation over a long document is the one place scrolling can jank.
function jumpTop() { scrollEl.scrollTo({ top: 0 }); }
function jumpBottom() { scrollEl.scrollTo({ top: scrollEl.scrollHeight }); }

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
  scrollEl.scrollTo({ top: f.top });
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

async function sendStack(ids) {
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

function openFileMenu(anchorEl, node) {
  fileMenuNode = node;
  const m = $("file-menu");
  const r = anchorEl.getBoundingClientRect();
  m.style.left = Math.min(r.left, window.innerWidth - 170) + "px";
  m.style.top = r.bottom + 4 + "px";
  m.classList.add("open");
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

function togglePane() {
  if ($("pty-pane").classList.contains("open")) closePane();
  else openPane();
}

/// Show the pane, building the terminal and starting the process the first
/// time. Every later open is a class flip and a refit.
async function openPane() {
  const pane = $("pty-pane");
  pane.classList.add("open");
  // The grid that docks the pane right is declared on `#main-wrap`, which
  // cannot see its child's class — see the `agent-right` block in index.html.
  document.body.classList.add("pane-open");
  try {
    // Ahead of the vendor load, because it decides the pane's geometry and a
    // terminal built into the wrong box would `fit()` to it. One round trip on
    // first open only; nothing here runs at boot.
    await loadAgentPrefs();
    await loadTerminalVendor();
    if (!pty.term) buildTerminal();
    fitPane();
    if (!pty.running) await startPaneProcess();
    pty.term.focus();
  } catch (e) {
    console.error(e);
    setPaneStatus(String(e.message || e));
  }
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
    pty.prefs = { position: "bottom", permission_mode: "accept-edits" };
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

  // The pane's height is a percentage of the window, so an OS window resize
  // changes the terminal's geometry with no keystroke involved.
  new ResizeObserver(() => fitPane()).observe($("pty-term"));
  if (!pty.listening) {
    pty.listening = true;
    listen("pty-data", (e) => { if (pty.term) pty.term.write(fromB64(e.payload)); });
    listen("pty-exit", (e) => {
      pty.running = false;
      $("pty-pane").classList.add("dead");
      const code = e.payload;
      setPaneStatus(code === null || code === undefined ? "exited" : `exited (${code})`);
      if (pty.term) pty.term.write("\r\n\x1b[2m[process exited — ⟳ to restart]\x1b[0m\r\n");
    });
  }
}

async function startPaneProcess() {
  setPaneStatus("");
  $("pty-pane").classList.remove("dead");
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
function fitPane() {
  const box = $("pty-term");
  if (!pty.fit || !box.clientHeight || !box.clientWidth) return;
  pty.fit.fit();
  if (pty.running) {
    invoke("pty_resize", { rows: pty.term.rows, cols: pty.term.cols }).catch(() => {});
  }
}

function setPaneStatus(text) {
  $("pty-status").textContent = text || "";
}

/// Kill and start again, in place. The button exists because the interesting
/// failure — `claude` not on the login shell's PATH — is one the user fixes
/// outside dreamd and then wants to retry without reopening anything.
async function restartPane() {
  try {
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
  // to warn about — write it and let the next start pick it up.
  if (!pty.running) { commitModeChange(); return; }
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
  $("pty-mode").onchange = () => stageModeChange();
  $("pty-mode-go").onclick = () => commitModeChange();
  $("pty-mode-cancel").onclick = () => cancelModeChange();
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

  $("btn-print").onclick = printDocument;
  $("btn-outline").onclick = toggleOutline;
  $("outline-close").onclick = closeOutline;
  $("btn-stack").onclick = toggleStack;
  $("stack-close").onclick = () => $("stack-panel").classList.remove("open");
  $("btn-send").onclick = () => sendStack([]);
  $("btn-send-all").onclick = () => sendStack([]);
  $("btn-send-selected").onclick = () => sendStack(checkedIds());
  $("annot-save").onclick = saveAnnot;
  $("annot-cancel").onclick = cancelAnnot;
  $("annot-delete").onclick = deleteHighlight;
  // Submits the annotation straight from the textarea (keyboard-only flow).
  // The global handler can't do this: it bails on editable targets.
  $("annot-text").addEventListener("keydown", (e) => {
    if (matchCombo(e, keymap.save_annotation)) {
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
    if (matchCombo(e, keymap.palette_next) || e.key === "ArrowDown") { e.preventDefault(); movePalette(1); }
    else if (matchCombo(e, keymap.palette_prev) || e.key === "ArrowUp") { e.preventDefault(); movePalette(-1); }
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
  wireTreeDrag();
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
      const claimed = ["palette-overlay", "annot-overlay", "confirm-overlay",
                       "settings-overlay", "file-menu", "find-bar", "pty-pane"]
        .some((id) => $(id).classList.contains("open"));
      closePalette();
      closeFileMenu();
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

    // Palette works from anywhere.
    if (matchCombo(e, keymap.palette)) { e.preventDefault(); openPalette(); return; }
    if (matchCombo(e, keymap.settings)) { e.preventDefault(); openSettings(); return; }
    // Above `isEditable`, like those two, and for the same reason: the pane is
    // a place you go to, not something you do to the document, so it must open
    // from the find bar and the annotation box as well as from the reader. It
    // is also the counterpart of the branch at the top of this handler — one
    // key, both directions.
    if (matchCombo(e, keymap.toggle_pane)) { e.preventDefault(); togglePane(); return; }

    // Bare-letter shortcuts must not fire while typing in a field.
    if (isEditable(e.target)) return;

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
    if (matchCombo(e, keymap.send_stack)) { e.preventDefault(); sendStack([]); return; }
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
  const combo = el.dataset.tipKey ? keymap[el.dataset.tipKey] : null;
  tip.innerHTML = escapeHtml(el.dataset.tip) +
    (combo ? `<span class="tt-key">${escapeHtml(combo)}</span>` : "");
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

function matchCombo(e, combo) {
  if (!combo) return false;
  const parts = combo.toLowerCase().split("+");
  const key = parts.pop();
  if (e.ctrlKey !== parts.includes("ctrl")) return false;
  if (e.shiftKey !== parts.includes("shift")) return false;
  if (e.altKey !== parts.includes("alt")) return false;
  if (e.metaKey !== parts.includes("meta")) return false;
  return (e.key || "").toLowerCase() === key;
}

/// Render a combo the way the platform writes it. Purely cosmetic — what gets
/// stored and matched is always the `Ctrl+Shift+X` form.
const MAC_SYMBOLS = { ctrl: "⌃", shift: "⇧", alt: "⌥", meta: "⌘" };
function displayCombo(combo) {
  if (!combo) return "—";
  if (!document.body.classList.contains("mac")) return combo;
  const parts = combo.split("+");
  const key = parts.pop();
  return parts.map((p) => MAC_SYMBOLS[p.toLowerCase()] || p + "+").join("") + key;
}

// ---- settings panel ------------------------------------------------------
// Three tabs over one payload from `get_settings`. Everything the panel writes
// goes through `set_config`, which is the same path `dreamd config set` takes,
// so a change made here and one made from the shell produce the same file.

const KEY_ACTIONS = [
  { id: "palette", label: "Open file palette", sub: "Fuzzy find markdown files" },
  { id: "palette_next", label: "Palette: next result" },
  { id: "palette_prev", label: "Palette: previous result" },
  { id: "highlight", label: "Highlight selection", sub: "Turn the selection into evidence and ask for an annotation" },
  { id: "save_annotation", label: "Save annotation", sub: "From inside the annotation box" },
  { id: "toggle_outline", label: "Toggle contents panel", sub: "Outline of the open document's headings" },
  { id: "toggle_tree", label: "Toggle file tree", sub: "Collapse or restore the sidebar" },
  { id: "toggle_view", label: "Toggle view mode", sub: "Hide the titlebar, sidebar and panels — Esc also exits" },
  { id: "toggle_stack", label: "Toggle stack panel" },
  { id: "toggle_pane", label: "Toggle Claude Code pane", sub: "A terminal docked under the document, running claude in this repo — the same key gets you back out of it" },
  { id: "jump_top", label: "Jump to top", sub: "Scroll the open document to the start" },
  { id: "jump_bottom", label: "Jump to bottom" },
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
function renderKeys() {
  const box = $("st-keys");
  box.innerHTML = "";
  const clashes = comboClashes();

  for (const action of KEY_ACTIONS) {
    const row = document.createElement("div");
    row.className = "st-row";
    const combo = keymap[action.id];
    row.innerHTML =
      `<span class="lbl">${escapeHtml(action.label)}` +
      (action.sub ? `<span class="sub">${escapeHtml(action.sub)}</span>` : "") +
      (shadowed("keymap." + action.id) ? `<span class="sub shadowed">overridden by .dreamd.toml in this repo</span>` : "") +
      `</span>`;
    const btn = document.createElement("button");
    btn.className = "combo" + (clashes.has(combo) ? " clash" : "");
    btn.textContent = displayCombo(combo);
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
    warn.textContent = "Two actions share a shortcut — the first one listed wins.";
    box.appendChild(warn);
  }
}

/// Combos bound to more than one action. The global handler is an if-chain, so
/// a duplicate is not an error — it just means the later action is unreachable.
function comboClashes() {
  const seen = new Map();
  const dupes = new Set();
  for (const a of KEY_ACTIONS) {
    const c = keymap[a.id];
    if (!c) continue;
    if (seen.has(c)) dupes.add(c);
    seen.set(c, a.id);
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

function comboFromEvent(e) {
  const key = e.key || "";
  if (MODIFIER_KEYS.includes(key)) return null;
  // `matchCombo` splits on "+", so a combo whose key is "+" can never match.
  if (key === "+") { toast("“+” can't be used as a shortcut key"); return null; }
  const parts = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.shiftKey) parts.push("Shift");
  if (e.altKey) parts.push("Alt");
  if (e.metaKey) parts.push("Meta");
  parts.push(key);
  return parts.join("+");
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
