// Fixture data for the stubbed `window.__TAURI__`.
//
// Everything here is derived from the corpus rather than recorded by hand, so
// it stays in step with the generator and needs nothing large committed. The
// one thing that *must* come from the real code is the rendered HTML: syntect
// emits an inline `style=` per token, which is most of what makes dreamd's
// payloads big, and hand-written HTML would understate the frontend's job.
// That gets produced by `cargo run --example render_doc`.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, dirname, basename } from "node:path";
import { fileURLToPath } from "node:url";

export const HARNESS = dirname(dirname(fileURLToPath(import.meta.url)));
export const ROOT = dirname(dirname(HARNESS));
export const UI = join(ROOT, "ui");
export const CORPUS = join(ROOT, "perf", "corpus", "generated");
// Deliberately a sibling of `generated/`, not a child. `gen.mjs` verifies the
// corpus by counting and hashing everything under `generated/`, so a derived
// cache living in there would make every run look stale — and the resulting
// regenerate would delete this cache, forcing a full re-render each time.
const RENDERED = join(ROOT, "perf", "corpus", "rendered");

/**
 * Pre-rendered HTML for a corpus document, produced by the real renderer and
 * cached on disk. Built in release: rendering 2MB of code-heavy markdown with
 * an unoptimized syntect takes minutes.
 */
export function renderedHtml(variant, size) {
  mkdirSync(RENDERED, { recursive: true });
  const src = join(CORPUS, "docs", `${variant}-${size}.md`);
  const out = join(RENDERED, `${variant}-${size}.html`);
  if (!existsSync(src)) {
    throw new Error(`corpus missing: ${src}\nrun: node perf/corpus/gen.mjs`);
  }
  if (!existsSync(out) || statSync(out).mtimeMs < statSync(src).mtimeMs) {
    process.stderr.write(`rendering ${basename(src)} via the real pipeline...\n`);
    execFileSync(
      "cargo",
      ["run", "--release", "--quiet", "--example", "render_doc", "--", src, out],
      { cwd: ROOT, stdio: ["ignore", "inherit", "inherit"] },
    );
  }
  return readFileSync(out, "utf8");
}

/** A FileNode tree matching what `list_markdown_files` returns. */
export function fileTree(repoFiles = 500) {
  const root = join(CORPUS, "repos", `repo-${repoFiles}`);
  const build = (dir, name) => {
    const children = [];
    for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
      a.name < b.name ? -1 : 1,
    )) {
      if (entry.name.startsWith(".")) continue;
      const p = join(dir, entry.name);
      if (entry.isDirectory()) children.push(build(p, entry.name));
      else if (entry.name.endsWith(".md")) {
        children.push({ name: entry.name, path: p, rel: p.slice(root.length + 1), is_dir: false, children: [] });
      }
    }
    return { name, path: dir, rel: dir.slice(root.length + 1), is_dir: true, children };
  };
  return build(root, `repo-${repoFiles}`);
}

/** Flat list of file nodes, for fuzzy_search results. */
export function flatFiles(tree) {
  const out = [];
  const walk = (n) => (n.is_dir ? n.children.forEach(walk) : out.push(n));
  walk(tree);
  return out;
}

/**
 * `Highlight` records as `get_highlights`/`reanchor` return them. Uses the
 * `rendered` quote — the whitespace-collapsed form the app actually stores —
 * so the DOM-walking cost in `applyHighlights` is measured against text that
 * behaves the way real highlights do.
 */
export function highlights(n, filePath) {
  const raw = JSON.parse(readFileSync(join(CORPUS, "highlights", `${n}.json`), "utf8"));
  return raw.map((h, i) => ({
    id: i + 1,
    file_path: filePath,
    line_start: 0,
    line_end: 0,
    quote: h.rendered,
    prefix: "",
    suffix: "",
    state: "active",
    annotation: null,
  }));
}

export function themeCss() {
  // Mirrors `theme::resolve`: base rules plus a palette. Reading theme.css
  // alone would leave every `var(--…)` unresolved and quietly change what the
  // Chromium scenarios are measuring.
  return (
    readFileSync(join(UI, "theme.css"), "utf8") +
    "\n" +
    readFileSync(join(UI, "themes", "dreamd.css"), "utf8")
  );
}

// Mirrors `Keymap::default()` in src-tauri/src/config.rs. Kept complete so the
// frontend takes the same code paths it does against the real backend.
export const KEYMAP = {
  palette: "Ctrl+F",
  palette_prev: "Ctrl+P",
  palette_next: "Ctrl+N",
  highlight: "Ctrl+H",
  send_stack: "Ctrl+Enter",
  toggle_stack: "Ctrl+O",
  toggle_outline: "Ctrl+I",
  toggle_tree: "Ctrl+B",
  toggle_view: "Ctrl+M",
  jump_top: "Home",
  jump_bottom: "End",
  copy_stack: "Ctrl+C",
  settings: "Ctrl+,",
  save_annotation: "Ctrl+Y",
  quick_highlight: true,
};
