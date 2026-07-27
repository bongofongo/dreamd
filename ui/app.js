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
  toggle_tree: "Ctrl+B",
  toggle_view: "Ctrl+M",
  jump_top: "Home",
  jump_bottom: "End",
  next_file: "]",
  prev_file: "[",
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

  try { keymap = await invoke("get_keymap"); } catch (e) {}
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
  const nameEl = $("repo-name");
  nameEl.textContent = hasRepo ? info.display || info.name || info.root : "no repo";
  nameEl.title = hasRepo ? info.root || "" : "";
}

// Split out so callers that already hold a fresh tree — `rebuild_index`
// returns one — don't have to ask Rust to walk the repo again for it.
function paintTree(root) {
  const tree = $("tree");
  tree.innerHTML = "";
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
  let html;
  try {
    html = await invoke("render_markdown", { path: currentFile });
  } catch (e) {
    contentEl.innerHTML = `<div class="empty">${escapeHtml(String(e))}</div>`;
    refreshOutline();
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
      a.onclick = (e) => { e.preventDefault(); scrollToFragment(href.slice(1)); };
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
        } else if (!insideRepo(target)) {
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
      if (insideRepo(abs)) img.src = "file://" + abs;
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

/// Is this already-normalized absolute path inside the open repo root?
///
/// A bare `startsWith(repoRoot)` is *not* containment: with a root of
/// `/w/notes` it also accepts `/w/notes-private/secret.md`, because the check
/// never reaches a path separator. The boundary has to be the separator, so
/// the test is "equal to the root, or under root + `/`". With no repo open
/// nothing is inside, which is what the image handler has always done.
function insideRepo(abs) {
  if (!repoRoot) return false;
  const root = repoRoot.replace(/\/+$/, "");
  return abs === root || abs.startsWith(root + "/");
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

function normalizePath(p) {
  const parts = [];
  for (const seg of p.split("/")) {
    if (seg === "..") parts.pop();
    else if (seg !== "." && seg !== "") parts.push(seg);
  }
  return "/" + parts.join("/");
}

// ---- highlights ----------------------------------------------------------
// Above this many placeable highlights, flattening the document once beats
// walking it per highlight. Below it, a walk that stops at the first match wins
// — flattening a 2MB document costs ~4ms whether there is one quote or five
// hundred. Measured crossover in the Chromium harness is around 5.
const SCAN_THRESHOLD = 4;

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
      if (!wrapByWalk(contentEl, quote, h.id)) addStaleChip(h);
      continue;
    }
    const p = locateInNodes(doc, quote);
    if (p) placements.push({ ...p, id: h.id });
    else addStaleChip(h); // active but unlocatable in the DOM
  }

  // Wrapping splits a text node, which invalidates every offset computed after
  // it — so apply back to front and nothing needs recomputing.
  placements.sort((a, b) => b.at - a.at);
  for (const p of placements) {
    const range = document.createRange();
    range.setStart(p.node, p.offset);
    range.setEnd(p.node, p.offset + p.length);
    wrapRange(range, p.id, false);
  }
}

// Wrap the first occurrence of `quote` that lies within a single text node,
// stopping the walk as soon as it is found. Returns true if it was placed.
function wrapByWalk(container, quote, id) {
  if (!quote) return false;
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  for (let node; (node = walker.nextNode()); ) {
    const idx = node.nodeValue.indexOf(quote);
    if (idx < 0) continue;
    const range = document.createRange();
    range.setStart(node, idx);
    range.setEnd(node, idx + quote.length);
    wrapRange(range, id, false);
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

function wrapRange(range, id, stale) {
  const mark = document.createElement("mark");
  mark.className = "hl" + (stale ? " stale" : "");
  mark.dataset.id = id;
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
  const mark = wrapRange(range, id, false);
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
async function refreshStack() {
  const pairs = await invoke("get_stack");
  const badge = $("stack-badge");
  badge.textContent = pairs.length;
  badge.classList.toggle("show", pairs.length > 0);
  const list = $("stack-list");
  list.innerHTML = "";
  if (pairs.length === 0) {
    list.innerHTML = `<div class="empty">No pairs yet. Select text and press ${keymap.highlight}.</div>`;
    return;
  }
  for (const p of pairs) {
    const el = document.createElement("div");
    el.className = "pair";
    const loc = p.highlight.line_start === p.highlight.line_end
      ? `L${p.highlight.line_start}`
      : `L${p.highlight.line_start}-${p.highlight.line_end}`;
    const rel = p.highlight.file_path.replace(/^.*\//, "");
    el.innerHTML =
      `<div class="top"><input type="checkbox" checked data-id="${p.highlight.id}" />` +
      `<span class="loc">${escapeHtml(rel)} · ${loc}${p.highlight.state === "stale" ? " · ⚠ stale" : ""}</span></div>` +
      `<div class="ev">${escapeHtml(p.highlight.quote.slice(0, 200))}</div>` +
      `<div class="an">${escapeHtml(p.annotation)}</div>`;
    const rm = document.createElement("button");
    rm.textContent = "remove";
    rm.style.marginTop = "6px";
    rm.onclick = async () => { await invoke("remove_pair", { id: p.highlight.id }); refreshStack(); };
    el.appendChild(rm);
    list.appendChild(el);
  }
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
    // works for a heading whose slug is not a valid CSS selector.
    btn.onclick = () => h.scrollIntoView({ block: "start" });
    frag.appendChild(btn);
  }
  list.appendChild(frag);
}

function checkedIds() {
  return [...document.querySelectorAll('#stack-list input[type="checkbox"]:checked')]
    .map((c) => Number(c.dataset.id));
}

async function sendStack(ids) {
  try {
    const res = await invoke("send_stack", { ids: ids || [] });
    toast(`${res.method}: ${res.detail}`);
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
  listen("file-added", async () => {
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
    if (e.payload && e.payload.path === currentFile) {
      currentFile = null;
      contentEl.innerHTML = `<div class="empty">File removed.</div>`;
      staleRail.innerHTML = "";
      refreshOutline();
    }
  });
  listen("theme-reloaded", () => loadTheme());

  // File → Open moved the tree root. Rust has already re-walked and re-read
  // config for the new repo by the time this fires, so this is the boot
  // sequence's data half over again — theme included, since a `.dreamd.toml`
  // in the new repo may name a different one. The payload is a file to open,
  // when the user picked a file rather than a folder.
  listen("repo-changed", async (e) => {
    try {
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
  // In highlight mode, finishing a text selection auto-starts the flow.
  contentEl.addEventListener("mouseup", () => {
    if (!highlightMode || pending) return;
    const sel = window.getSelection();
    if (sel && !sel.isCollapsed && sel.toString().trim()) triggerHighlight();
  });
  // Click an existing highlight to edit / re-add / delete it.
  contentEl.addEventListener("click", (e) => {
    const m = e.target.closest && e.target.closest("mark.hl");
    if (m && contentEl.contains(m)) { e.preventDefault(); openEditHighlight(Number(m.dataset.id)); }
  });

  $("btn-outline").onclick = toggleOutline;
  $("outline-close").onclick = () => $("outline-panel").classList.remove("open");
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

  wireSettings();
}

function isEditable(el) {
  if (!el) return false;
  return /^(input|textarea|select)$/i.test(el.tagName || "") || el.isContentEditable;
}

function wireKeys() {
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      // Recording a keybind swallows Escape as "cancel", not "close panel".
      if (cancelRecording()) { e.preventDefault(); return; }
      // Escape is also the way out of view mode, because view mode hides the
      // titlebar and leaves nothing to click. It is the *last* claim on the
      // key though: if any overlay or the file menu is open, this Escape
      // closes that and view mode survives.
      const claimed = ["palette-overlay", "annot-overlay", "confirm-overlay",
                       "settings-overlay", "file-menu"]
        .some((id) => $(id).classList.contains("open"));
      closePalette();
      closeFileMenu();
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
  { id: "jump_top", label: "Jump to top", sub: "Scroll the open document to the start" },
  { id: "jump_bottom", label: "Jump to bottom" },
  { id: "next_file", label: "Next file", sub: "Move through the sidebar's order without touching the tree — wraps at the ends" },
  { id: "prev_file", label: "Previous file" },
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
