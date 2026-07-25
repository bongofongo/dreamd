"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

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
let keymap = {
  palette: "Ctrl+F",
  palette_prev: "Ctrl+P",
  palette_next: "Ctrl+N",
  highlight: "Ctrl+H",
  send_stack: "Ctrl+Enter",
  toggle_stack: "Ctrl+O",
  copy_stack: "Ctrl+C",
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
  try {
    const info = await invoke("repo_info");
    repoRoot = info.root || "";
    const nameEl = $("repo-name");
    nameEl.textContent = info.display || info.name || info.root;
    nameEl.title = info.root || "";
  } catch (e) { console.error(e); }
  perf.at("ipc_repo_info");

  try { keymap = await invoke("get_keymap"); } catch (e) {}
  $("search-hint").textContent = `Press ${keymap.palette} to search`;
  perf.at("ipc_keymap");

  await loadTheme();
  perf.at("ipc_theme");
  await loadTree();
  perf.at("ipc_tree");
  wireEvents();
  wireKeys();
  wireUi();
  wireTooltips();
  perf.at("wired");

  // nvim-style: `dreamd file.md` opens the file on load.
  try {
    const f = await invoke("initial_file");
    if (f) await openFile(f);
  } catch (e) { console.error(e); }
  perf.at("first_paint");
}

async function loadTheme() {
  try {
    const css = await invoke("get_theme_css");
    $("user-theme").textContent = css;
  } catch (e) { console.error(e); }
}

// ---- file tree -----------------------------------------------------------
async function loadTree() {
  paintTree(await invoke("list_markdown_files"));
}

// Split out so callers that already hold a fresh tree — `rebuild_index`
// returns one — don't have to ask Rust to walk the repo again for it.
function paintTree(root) {
  const tree = $("tree");
  tree.innerHTML = "";
  // Render the root's children directly (skip the root dir node itself).
  for (const child of root.children) tree.appendChild(renderNode(child));
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

// ---- open / render -------------------------------------------------------
async function openFile(path) {
  currentFile = path;
  markActiveInTree(path);
  await renderCurrent(false);
}

async function renderCurrent(preserveScroll) {
  if (!currentFile) return;
  const t0 = perf.now();
  const prevScroll = preserveScroll ? scrollEl.scrollTop : 0;
  let html;
  try {
    html = await invoke("render_markdown", { path: currentFile });
  } catch (e) {
    contentEl.innerHTML = `<div class="empty">${escapeHtml(String(e))}</div>`;
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
  const highlights = preserveScroll
    ? await invoke("reanchor", { path: currentFile })
    : await invoke("get_highlights", { path: currentFile });
  perf.span(preserveScroll ? "ipc_reanchor" : "ipc_get_highlights", t);

  t = perf.now();
  applyHighlights(highlights);
  perf.span("apply_highlights", t);

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
      a.onclick = (e) => {
        e.preventDefault();
        const t = contentEl.querySelector(href) || document.getElementById(href.slice(1));
        if (t) t.scrollIntoView();
      };
    } else {
      // relative path -> only navigate to markdown inside the repo; other
      // relative targets are dropped (never handed to the OS opener).
      a.onclick = (e) => {
        e.preventDefault();
        const base = currentFile.replace(/[^\/]*$/, "");
        const target = normalizePath(base + href.split("#")[0]);
        if (/\.(md|markdown|mdown|mkd)$/i.test(target)) openFile(target);
        else toast("Ignored non-markdown local link");
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
      if (repoRoot && abs.startsWith(repoRoot)) img.src = "file://" + abs;
      else img.removeAttribute("src");
    }
  });
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
  const sc = document.body.classList.contains("mac") ? "⌃Y" : "Ctrl+Y";
  $("annot-save").textContent = (mode === "edit" ? "Save" : "Add to stack") + "  " + sc;
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

function toggleStack() { $("stack-panel").classList.toggle("open"); refreshStack(); }

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
      await renderCurrent(true);
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
    }
  });
  listen("theme-reloaded", () => loadTheme());
}

function wireUi() {
  $("btn-collapse").onclick = () => document.body.classList.add("nav-collapsed");
  $("btn-expand").onclick = () => document.body.classList.remove("nav-collapsed");
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

  $("btn-stack").onclick = toggleStack;
  $("stack-close").onclick = () => $("stack-panel").classList.remove("open");
  $("btn-send").onclick = () => sendStack([]);
  $("btn-send-all").onclick = () => sendStack([]);
  $("btn-send-selected").onclick = () => sendStack(checkedIds());
  $("annot-save").onclick = saveAnnot;
  $("annot-cancel").onclick = cancelAnnot;
  $("annot-delete").onclick = deleteHighlight;
  // Ctrl+Y submits the annotation straight from the textarea (keyboard-only flow).
  $("annot-text").addEventListener("keydown", (e) => {
    if (e.ctrlKey && !e.metaKey && !e.altKey && (e.key || "").toLowerCase() === "y") {
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
}

function isEditable(el) {
  if (!el) return false;
  return /^(input|textarea|select)$/i.test(el.tagName || "") || el.isContentEditable;
}

function wireKeys() {
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closePalette();
      closeFileMenu();
      if ($("annot-overlay").classList.contains("open")) cancelAnnot();
      if ($("confirm-overlay").classList.contains("open")) closeConfirm();
      return;
    }
    // While an overlay is open, let its own inputs handle keys.
    if ($("palette-overlay").classList.contains("open") ||
        $("annot-overlay").classList.contains("open") ||
        $("confirm-overlay").classList.contains("open")) return;

    // Palette works from anywhere.
    if (matchCombo(e, keymap.palette)) { e.preventDefault(); openPalette(); return; }

    // Bare-letter shortcuts must not fire while typing in a field.
    if (isEditable(e.target)) return;

    // `h` (or the configured highlight key) turns the current selection into a
    // dreamd highlight and prompts for an annotation. It does NOT toggle mode.
    if (matchCombo(e, "h") || matchCombo(e, keymap.highlight)) {
      e.preventDefault();
      triggerHighlight();
      return;
    }
    if (matchCombo(e, keymap.toggle_stack)) { e.preventDefault(); toggleStack(); return; }
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
