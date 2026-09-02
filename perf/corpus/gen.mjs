#!/usr/bin/env node
//
// Deterministic corpus generator for the dreamd perf harness.
//
//   node perf/corpus/gen.mjs [--stress] [--force] [--check]
//
// Everything here is driven by a seeded PRNG and fixed word banks, so two runs
// on two machines produce byte-identical output. That is the whole point: a
// benchmark number is only comparable if the input is.
//
// Writes to perf/corpus/generated/ (gitignored) and records every file's size
// and sha256 in perf/corpus/manifest.json (committed). If the manifest already
// matches what's on disk, generation is skipped — the perf tiers call this on
// every run and regenerating ~11MB each time would be pure waste.
//
// No dependencies, and deliberately no `Math.random`, no `Date`, no locale-
// dependent formatting.

import { mkdirSync, writeFileSync, readFileSync, readdirSync, existsSync, rmSync } from "node:fs";
import { createHash } from "node:crypto";
import { join, dirname, relative } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, "generated");
const MANIFEST = join(HERE, "manifest.json");

/** Bump when a generator change invalidates previously-recorded numbers. */
const GENERATOR = 3;

const argv = new Set(process.argv.slice(2));
const STRESS = argv.has("--stress");
const FORCE = argv.has("--force");
const CHECK = argv.has("--check");

// ---- deterministic randomness --------------------------------------------

/** mulberry32 — small, fast, and identical everywhere. */
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const pick = (r, xs) => xs[Math.floor(r() * xs.length) % xs.length];
const int = (r, lo, hi) => lo + Math.floor(r() * (hi - lo + 1));

// ---- word banks ----------------------------------------------------------

const WORDS = `the quick brown fox jumps over lazy dog markdown reader highlight
annotation stack send loop terminal webview render parse anchor quote prefix
suffix theme palette fuzzy search index walk watcher debounce cache latency
throughput frame paint layout reflow allocation buffer stream token syntax
document heading paragraph list table code block inline link image escape
sanitize repository commit branch session editor window pane tmux clipboard
config keymap binding scroll selection range node tree leaf branch depth`
  .split(/\s+/)
  .filter(Boolean);

const TITLES = [
  "Architecture", "Rendering", "Anchoring", "The send loop", "Configuration",
  "Themes", "Search", "File watching", "Security notes", "Known limits",
  "Performance", "Glossary", "Open questions", "Design intent", "Trade-offs",
];

const CODE = {
  rust: `pub fn locate(source: &str, quote: &str) -> Option<usize> {
    if quote.trim().is_empty() {
        return None;
    }
    source.find(quote)
}`,
  javascript: `function applyHighlights(list) {
  for (const h of list) {
    const range = findRange(h.quote);
    if (range) wrapRange(range, h.id);
  }
}`,
  python: `def strip_ws(source):
    out, index = [], []
    for i, ch in enumerate(source):
        if not ch.isspace():
            out.append(ch)
            index.append(i)
    return "".join(out), index`,
  go: `func Walk(root string) ([]string, error) {
	var out []string
	err := filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
		if err == nil && !d.IsDir() {
			out = append(out, p)
		}
		return err
	})
	return out, err
}`,
  c: `static size_t line_at(const char *src, size_t idx) {
    size_t n = 1;
    for (size_t i = 0; i < idx; i++)
        if (src[i] == '\\n') n++;
    return n;
}`,
  sql: `SELECT path, count(*) AS hits
FROM highlights
WHERE stale = 0
GROUP BY path
ORDER BY hits DESC
LIMIT 20;`,
  toml: `[keymap]
palette = "cmd+p"
highlight = "h"
send = "cmd+enter"`,
  bash: `#!/usr/bin/env bash
set -euo pipefail
for f in "$@"; do
  printf '%s\\t%s\\n' "$(wc -c <"$f")" "$f"
done`,
};
const LANGS = Object.keys(CODE);

// ---- images --------------------------------------------------------------
//
// Real PNG bytes rather than a placeholder, because a corpus image that cannot
// decode measures nothing: `measureImage` skips a broken image deliberately
// (`width: calc(0px * z)` would collapse the alt text), so a fake file would
// exercise the one path that does the least work.
//
// The encoder writes **stored** deflate blocks instead of calling `zlib`. The
// whole premise of this file is that two machines produce identical bytes, and
// zlib's compressed output is a property of whichever zlib Node was linked
// against — a difference there would not corrupt anything, but it would make
// the committed manifest disagree with a correct clone and regenerate ~11MB on
// every run. A stored block is defined by the format, not by an encoder.
//
// That costs size, which is why every corpus image is tiny. Their dimensions
// differ so that `--img-w` has something to scale differently, and nothing
// downstream decodes them for their content: the Chromium scenarios are served
// pre-rendered HTML, so these exist for the Rust render path, the frontend's
// per-image work, and anyone opening the corpus in a real window.

const CRC_TABLE = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();

function crc32(buf) {
  let c = ~0;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return ~c >>> 0;
}

function adler32(buf) {
  let a = 1;
  let b = 0;
  for (const byte of buf) {
    a = (a + byte) % 65521;
    b = (b + a) % 65521;
  }
  return ((b << 16) | a) >>> 0;
}

/** A zlib stream of stored (BTYPE=00) blocks — no compression, no encoder. */
function zlibStored(data) {
  const parts = [Buffer.from([0x78, 0x01])];
  const MAX = 65535;
  for (let off = 0; off < data.length || off === 0; off += MAX) {
    const chunk = data.subarray(off, off + MAX);
    const last = off + MAX >= data.length ? 1 : 0;
    const head = Buffer.alloc(5);
    head[0] = last;
    head.writeUInt16LE(chunk.length, 1);
    head.writeUInt16LE(~chunk.length & 0xffff, 3);
    parts.push(head, Buffer.from(chunk));
    if (last) break;
  }
  const out = Buffer.concat(parts);
  const tail = Buffer.alloc(4);
  tail.writeUInt32BE(adler32(data), 0);
  return Buffer.concat([out, tail]);
}

function pngChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([len, body, crc]);
}

/**
 * An 8-bit RGB PNG of `w`×`h`, painted by `px(x, y) -> [r, g, b]`.
 *
 * Filter byte 0 (None) on every scanline: the point is bytes a decoder accepts,
 * not bytes a decoder finds small.
 */
function png(w, h, px) {
  const raw = Buffer.alloc(h * (1 + w * 3));
  let i = 0;
  for (let y = 0; y < h; y++) {
    raw[i++] = 0;
    for (let x = 0; x < w; x++) {
      const [r, g, b] = px(x, y);
      raw[i++] = r;
      raw[i++] = g;
      raw[i++] = b;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // colour type: truecolour
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", zlibStored(raw)),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

/**
 * The corpus image set. Four files, deliberately few: a document referencing
 * the same diagram twenty times is what a real repo looks like, and twenty
 * distinct files would only make the corpus bigger without exercising anything
 * new. The aspect ratios are the spread that matters — square, landscape,
 * portrait and a one-pixel-tall rule — because that is what decides how a
 * zoomed image reflows the prose around it.
 */
const IMAGES = [
  { name: "icon-16.png", w: 16, h: 16 },
  { name: "figure-96x64.png", w: 96, h: 64 },
  { name: "portrait-48x120.png", w: 48, h: 120 },
  { name: "rule-256x2.png", w: 256, h: 2 },
];

/** A checkerboard with a solid border — visibly wrong if a decoder mangles it. */
function checker(w, h) {
  return png(w, h, (x, y) => {
    if (x === 0 || y === 0 || x === w - 1 || y === h - 1) return [220, 60, 60];
    const on = ((x >> 2) + (y >> 2)) % 2 === 0;
    return on ? [235, 235, 235] : [70, 90, 140];
  });
}

// ---- block builders ------------------------------------------------------

function sentence(r, n = int(r, 8, 20)) {
  const w = Array.from({ length: n }, () => pick(r, WORDS));
  w[0] = w[0][0].toUpperCase() + w[0].slice(1);
  return w.join(" ") + pick(r, [".", ".", ".", "?", "!"]);
}

function paragraph(r) {
  return Array.from({ length: int(r, 3, 7) }, () => sentence(r)).join(" ");
}

function heading(r, depth) {
  return `${"#".repeat(depth)} ${pick(r, TITLES)} ${int(r, 1, 99)}`;
}

function proseBlock(r) {
  const roll = r();
  if (roll < 0.12) return heading(r, int(r, 2, 4));
  if (roll < 0.22) {
    const n = int(r, 3, 8);
    return Array.from({ length: n }, () => `- ${sentence(r, int(r, 4, 12))}`).join("\n");
  }
  if (roll < 0.3) {
    const n = int(r, 2, 5);
    return Array.from({ length: n }, (_, i) => `${i + 1}. ${sentence(r, int(r, 4, 12))}`).join("\n");
  }
  if (roll < 0.36) return `> ${sentence(r)}\n> ${sentence(r)}`;
  if (roll < 0.42) {
    // Inline markup — this is what makes single-text-node highlight matching
    // fail, so the corpus has to contain plenty of it.
    return `${sentence(r, 6)} \`${pick(r, WORDS)}\` ${sentence(r, 5)} **${pick(r, WORDS)}** and *${pick(r, WORDS)}*, see [${pick(r, WORDS)}](https://example.com/${pick(r, WORDS)}).`;
  }
  return paragraph(r);
}

function codeBlock(r) {
  const lang = pick(r, LANGS);
  const body = CODE[lang];
  const reps = int(r, 1, 4);
  return "```" + lang + "\n" + Array.from({ length: reps }, () => body).join("\n\n") + "\n```";
}

function tableBlock(r) {
  const cols = int(r, 4, 9);
  const rows = int(r, 5, 25);
  const head = Array.from({ length: cols }, () => pick(r, WORDS));
  const sep = Array.from({ length: cols }, () => "---");
  const body = Array.from({ length: rows }, () =>
    Array.from({ length: cols }, () => pick(r, WORDS) + " " + int(r, 1, 9999)),
  );
  return [head, sep, ...body].map((c) => `| ${c.join(" | ")} |`).join("\n");
}

/**
 * A block bearing one image, in the shapes a real document uses.
 *
 * The path is `img/<name>`, a plain relative reference with no `../` in it —
 * climbing out of the root is the one thing the containment guard refuses, and
 * a corpus where every image sits on that boundary would benchmark the refusal
 * rather than the load. Two shapes below do climb out, on purpose and rarely,
 * so the refusal is represented without dominating.
 */
function imageBlock(r) {
  const img = pick(r, IMAGES);
  const alt = `${pick(r, WORDS)} ${pick(r, WORDS)}`;
  const roll = r();
  // A figure: the common case, an image alone in its own block.
  if (roll < 0.4) return `![${alt}](img/${img.name})`;
  // With a title attribute — a second string pulldown-cmark has to carry.
  if (roll < 0.55) return `![${alt}](img/${img.name} "${sentence(r, 5)}")`;
  // Inline, mid-sentence. This is the one that interacts with highlighting:
  // the image is a node inside the paragraph a reader selects across.
  if (roll < 0.72) return `${sentence(r, 8)} ![${alt}](img/${img.name}) ${sentence(r, 6)}`;
  // Linked, which is an `<a>` wrapping an `<img>` — two intercepts on one node.
  if (roll < 0.84) return `[![${alt}](img/${img.name})](https://example.com/${pick(r, WORDS)})`;
  // Remote. Never fetched by the guard's rules, but it is what a README holds.
  if (roll < 0.92) return `![${alt}](https://example.com/${pick(r, WORDS)}.png)`;
  // Refused: enough `../` to leave the repo. The frontend strips the `src`.
  return `![${alt}](../../../${pick(r, WORDS)}.png)`;
}

const VARIANTS = {
  // Isolates pulldown-cmark: no fenced blocks at all.
  prose: (r) => proseBlock(r),
  // Isolates syntect, the dominant render cost.
  code: (r) => (r() < 0.75 ? codeBlock(r) : proseBlock(r)),
  // Targets the per-table overflow-x scroll containers in theme.css.
  table: (r) => (r() < 0.7 ? tableBlock(r) : proseBlock(r)),
  // The headline number: what a real document looks like.
  mixed: (r) => {
    const roll = r();
    if (roll < 0.15) return codeBlock(r);
    if (roll < 0.25) return tableBlock(r);
    return proseBlock(r);
  },
  // Prose carrying images at about the density of a documentation page — one
  // per dozen-or-so blocks, not a gallery. Appended **last** deliberately:
  // `generate()` hands out seeds in `Object.keys` order, so a variant inserted
  // anywhere above this line would renumber every document below it and change
  // bytes that every recorded benchmark number was measured against.
  images: (r) => (r() < 0.08 ? imageBlock(r) : proseBlock(r)),
};

const SIZES = {
  "8k": 8 * 1024,
  "128k": 128 * 1024,
  "512k": 512 * 1024,
  "2m": 2 * 1024 * 1024,
};
const STRESS_SIZES = { "8m": 8 * 1024 * 1024 };

/**
 * Build a document of at least `target` bytes. We never truncate mid-document:
 * cutting a fenced block or table in half would produce markdown that isn't
 * representative of anything. Actual size is recorded in the manifest.
 */
function buildDoc(variant, target, seed) {
  const r = rng(seed);
  const parts = [`# ${variant} corpus — ${target} bytes target`, ""];
  let bytes = 0;
  while (bytes < target) {
    const block = VARIANTS[variant](r);
    parts.push(block, "");
    bytes += Buffer.byteLength(block) + 2;
  }
  return parts.join("\n");
}

// ---- highlight sets ------------------------------------------------------

/**
 * Sample quotes out of a real generated document, with the 40 characters of
 * source either side captured as prefix/suffix.
 *
 * Three fields per entry, because `locate` has three tiers and which one you
 * land in dominates the cost:
 *
 *   `quote`     the exact source slice. Hits tier 1/2 (`source.find`) and is
 *               cheap. This is NOT what the app produces.
 *   `rendered`  the same text with whitespace collapsed — an approximation of
 *               what `window.getSelection().toString()` actually yields, since
 *               the user selects from the *rendered* DOM, not the source. This
 *               misses tiers 1 and 2 and falls through to `locate_normalized`,
 *               which is the realistic path today and roughly 6x the cost.
 *   `prefix`/`suffix`  context, from the source either side of the quote.
 *   `lineStart`/`lineEnd`  where the quote was actually sampled from — the
 *               ground truth `src-tauri/examples/locate_check.rs` checks
 *               `markdown::locate` against. It has to be recorded here because
 *               it is not recoverable afterwards: the corpus repeats whole
 *               blocks, so searching for the quote finds the first copy, which
 *               is usually not the one it came from. Measured to the first and
 *               last non-whitespace character, matching what `locate` reports.
 *
 * Only quotes that span a newline are kept, so `rendered` is guaranteed to
 * differ from `quote` — otherwise the fixture would silently benchmark the
 * cheap path and understate the real cost.
 */
function buildHighlights(doc, n, seed) {
  const r = rng(seed);
  const out = [];
  const guard = n * 400;
  // Byte offset of each line start, so line lookup is a binary search rather
  // than a scan of a 2MB document per highlight.
  const lineStarts = [0];
  for (let i = doc.indexOf("\n"); i !== -1; i = doc.indexOf("\n", i + 1)) lineStarts.push(i + 1);
  const lineAt = (idx) => {
    let lo = 0;
    let hi = lineStarts.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (lineStarts[mid] <= idx) lo = mid + 1;
      else hi = mid;
    }
    return lo; // 1-based
  };

  for (let i = 0; out.length < n && i < guard; i++) {
    const start = int(r, 200, Math.max(201, doc.length - 400));
    const len = int(r, 30, 180);
    const quote = doc.slice(start, start + len);
    // Skip fence markers and pipes: a quote straddling a code fence or table
    // row isn't something a user could select in the rendered view.
    if (quote.includes("```") || quote.includes("|") || !quote.trim()) continue;
    // Must span a line break, or `rendered` would equal `quote`.
    if (!/\n/.test(quote)) continue;
    const lead = quote.length - quote.trimStart().length;
    const trail = quote.length - quote.trimEnd().length;
    out.push({
      quote,
      rendered: quote.replace(/\s+/g, " ").trim(),
      prefix: doc.slice(Math.max(0, start - 40), start),
      suffix: doc.slice(start + len, start + len + 40),
      lineStart: lineAt(start + lead),
      lineEnd: lineAt(start + len - trail - 1),
    });
  }
  if (out.length < n) throw new Error(`only sampled ${out.length}/${n} highlights`);
  return out;
}

// ---- synthetic repos -----------------------------------------------------

function buildRepo(files, seed) {
  const r = rng(seed);
  const out = [];
  const dirs = ["docs", "notes", "src/adr", "vendor/lib/docs", "team/eng", "archive/2024"];
  for (let i = 0; i < files; i++) {
    const d = pick(r, dirs);
    const depth = int(r, 0, 2);
    const nested = Array.from({ length: depth }, () => pick(r, WORDS)).join("/");
    const rel = join(d, nested, `${pick(r, WORDS)}-${i}.md`);
    out.push({ rel, body: buildDoc("mixed", 1500, seed + i) });
  }
  return out;
}

// ---- write + manifest ----------------------------------------------------

const written = [];
function emit(rel, contents) {
  const abs = join(OUT, rel);
  mkdirSync(dirname(abs), { recursive: true });
  writeFileSync(abs, contents);
  written.push({
    path: rel,
    bytes: Buffer.byteLength(contents),
    sha256: createHash("sha256").update(contents).digest("hex"),
  });
}

/**
 * One hash over every generated file. Committing 5,500 individual hashes would
 * be a megabyte of churn in the repo for no benefit — a single digest answers
 * the only question that matters ("is what's on disk what this generator
 * produces?") and still catches a single corrupted byte anywhere.
 */
function digestOf(files) {
  const h = createHash("sha256");
  for (const f of [...files].sort((a, b) => (a.path < b.path ? -1 : 1))) {
    h.update(`${f.path}:${f.sha256}\n`);
  }
  return h.digest("hex");
}

/** Hash everything currently under OUT, in the same shape `emit` records. */
function scanDisk(dir = OUT, prefix = "") {
  const out = [];
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) out.push(...scanDisk(join(dir, entry.name), rel));
    else {
      const buf = readFileSync(join(dir, entry.name));
      out.push({
        path: rel,
        bytes: buf.length,
        sha256: createHash("sha256").update(buf).digest("hex"),
      });
    }
  }
  return out;
}

function manifestMatches() {
  if (!existsSync(MANIFEST)) return false;
  let prev;
  try {
    prev = JSON.parse(readFileSync(MANIFEST, "utf8"));
  } catch {
    return false;
  }
  // A generator change alters what the files *should* contain, but the stale
  // files on disk still match the stale manifest — so the version has to be
  // checked explicitly or a regenerate would be silently skipped.
  if (prev.generator !== GENERATOR) return false;
  if (prev.stress !== STRESS) return false;
  const onDisk = scanDisk();
  if (onDisk.length !== prev.fileCount) return false;
  return digestOf(onDisk) === prev.digest;
}

function generate() {
  rmSync(OUT, { recursive: true, force: true });

  const sizes = { ...SIZES, ...(STRESS ? STRESS_SIZES : {}) };
  let seed = 1;
  const docs = {};
  for (const variant of Object.keys(VARIANTS)) {
    for (const [label, target] of Object.entries(sizes)) {
      const doc = buildDoc(variant, target, seed++);
      const rel = join("docs", `${variant}-${label}.md`);
      emit(rel, doc);
      docs[`${variant}-${label}`] = doc;
    }
  }

  // The image set the `images` variant references. Written once and shared by
  // every size, because the documents reference them by the same relative path
  // and `docs/img/` is where that path lands.
  for (const img of IMAGES) emit(join("docs", "img", img.name), checker(img.w, img.h));

  // Highlight sets are sampled from the 2MB mixed document — the size where
  // the per-highlight cost in locate_normalized actually bites.
  const base = docs["mixed-2m"];
  for (const n of [1, 10, 100, 500]) {
    emit(join("highlights", `${n}.json`), JSON.stringify(buildHighlights(base, n, 9000 + n), null, 2) + "\n");
  }

  for (const n of [10, 500, 5000]) {
    const repo = `repos/repo-${n}`;
    // A `.git` marker so `resolve_repo_root` stops here instead of walking up
    // into dreamd's own repo and benchmarking the wrong tree.
    emit(join(repo, ".git", "HEAD"), "ref: refs/heads/main\n");
    emit(join(repo, ".gitignore"), "node_modules/\ntarget/\n");
    for (const f of buildRepo(n, 20000 + n)) emit(join(repo, f.rel), f.body);
  }

  written.sort((a, b) => (a.path < b.path ? -1 : 1));
  const isRepo = (p) => p.startsWith("repos/");
  const manifest = {
    generator: GENERATOR,
    stress: STRESS,
    fileCount: written.length,
    totalBytes: written.reduce((n, f) => n + f.bytes, 0),
    digest: digestOf(written),
    // Per-file hashes for the documents and highlight sets, because those are
    // the inputs whose exact bytes determine every benchmark number. The
    // synthetic repos only matter in aggregate — they're walked and counted,
    // never parsed — so they're summarized rather than enumerated.
    // `docs` stays the markdown alone. The images are listed beside it rather
    // than folded in: they are an input whose bytes matter the same way, but a
    // reader comparing two manifests wants to see at a glance which half moved.
    docs: written.filter((f) => f.path.startsWith("docs/") && f.path.endsWith(".md")),
    images: written.filter((f) => f.path.startsWith("docs/img/")),
    highlights: written.filter((f) => f.path.startsWith("highlights/")),
    repos: Object.entries(
      written.filter((f) => isRepo(f.path)).reduce((acc, f) => {
        const name = f.path.split("/")[1];
        acc[name] ??= { files: 0, bytes: 0 };
        acc[name].files += 1;
        acc[name].bytes += f.bytes;
        return acc;
      }, {}),
    ).map(([name, v]) => ({ name, ...v })),
  };
  writeFileSync(MANIFEST, JSON.stringify(manifest, null, 2) + "\n");
  return manifest;
}

// ---- main ----------------------------------------------------------------

if (CHECK) {
  const ok = manifestMatches();
  console.log(ok ? "corpus: up to date" : "corpus: stale or missing");
  process.exit(ok ? 0 : 1);
}

if (!FORCE && manifestMatches()) {
  const prev = JSON.parse(readFileSync(MANIFEST, "utf8"));
  console.log(`corpus: up to date (${prev.fileCount} files, ${(prev.totalBytes / 1048576).toFixed(1)} MB)`);
} else {
  const m = generate();
  console.log(
    `corpus: generated ${m.fileCount} files, ${(m.totalBytes / 1048576).toFixed(1)} MB` +
      (STRESS ? " (incl. 8MB stress docs)" : ""),
  );
  console.log(`corpus: manifest -> ${relative(process.cwd(), MANIFEST)}`);
}
