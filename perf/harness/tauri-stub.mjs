// Builds the `window.__TAURI__` shim that gets injected before `ui/app.js`
// runs, plus the Playwright plumbing to load the real UI around it.
//
// WHAT THIS DOES AND DOESN'T TELL YOU
//
// Chromium is not WKWebView. Layout, style recalc, and JS engine behavior all
// differ, so absolute numbers from this harness are NOT the app's real timings.
// What it does give is a fast, scriptable, deterministic way to see *relative*
// movement in the frontend's own work — innerHTML swap cost, TreeWalker cost,
// long tasks during scroll — which is exactly what the quick tier needs.
//
// Real-webview truth comes from `perf/scripts/loop.sh` and Instruments.
//
// IPC responses are served from fixtures with a configurable artificial delay
// so backend cost doesn't smear into the frontend numbers being measured.

import { chromium } from "playwright";
import { pathToFileURL } from "node:url";
import { join } from "node:path";
import * as fx from "./lib/fixtures.mjs";

/**
 * @param {object} opts
 * @param {string} [opts.variant]   corpus variant to serve as the open document
 * @param {string} [opts.size]      corpus size label
 * @param {number} [opts.highlights] how many highlights the store holds
 * @param {number} [opts.ipcDelayMs] simulated backend latency per invoke
 * @param {boolean} [opts.openInitial] whether `initial_file` returns a path
 */
export async function launch(opts = {}) {
  const {
    variant = "mixed",
    size = "2m",
    highlights = 0,
    ipcDelayMs = 0,
    openInitial = true,
    repoFiles = 500,
  } = opts;

  const docPath = join(fx.CORPUS, "docs", `${variant}-${size}.md`);
  const tree = fx.fileTree(repoFiles);

  const responses = {
    perf_enabled: false,
    repo_info: { root: fx.CORPUS, name: "corpus", display: "~/corpus" },
    get_keymap: fx.KEYMAP,
    get_theme: { css: fx.themeCss(), mode: "dark", scheme: "dark", syntax_theme: null },
    list_markdown_files: tree,
    initial_file: openInitial ? docPath : null,
    render_markdown: fx.renderedHtml(variant, size),
    get_highlights: highlights ? fx.highlights(highlights, docPath) : [],
    reanchor: highlights ? fx.highlights(highlights, docPath) : [],
    get_stack: [],
    stack_query_text: "",
    fuzzy_search: fx.flatFiles(tree).slice(0, 200),
  };

  const browser = await chromium.launch({
    args: [
      // Frame timings are the whole point of the scroll scenario; without this
      // Chromium throttles rendering when the window isn't foregrounded.
      "--disable-background-timer-throttling",
      "--disable-renderer-backgrounding",
      "--disable-backgrounding-occluded-windows",
    ],
  });
  // Pinned so scenarios are comparable across runs: app.js reads
  // prefers-color-scheme before first paint, and Chromium's default is light.
  // Nothing timed depends on which appearance it is — but it should be a
  // decision rather than a browser default that can change under a baseline.
  const page = await browser.newPage({
    viewport: { width: 1200, height: 850 },
    colorScheme: "dark",
  });

  await page.addInitScript(
    ({ responses, ipcDelayMs }) => {
      const listeners = new Map();
      const wait = (ms) => (ms ? new Promise((r) => setTimeout(r, ms)) : Promise.resolve());

      window.__PERF__ = { invokes: [], longTasks: [] };

      window.__TAURI__ = {
        core: {
          async invoke(cmd, args) {
            const t0 = performance.now();
            await wait(ipcDelayMs);
            window.__PERF__.invokes.push({ cmd, ms: performance.now() - t0 });
            if (!(cmd in responses)) return null;
            return responses[cmd];
          },
        },
        event: {
          async listen(name, cb) {
            if (!listeners.has(name)) listeners.set(name, []);
            listeners.get(name).push(cb);
            return () => {};
          },
        },
      };

      // Lets scenarios fire watcher events the way the Rust side would.
      window.__EMIT__ = (name, payload) => {
        for (const cb of listeners.get(name) || []) cb({ payload });
      };

      // Long tasks are the machine-readable form of "it stuttered".
      try {
        new PerformanceObserver((list) => {
          for (const e of list.getEntries()) {
            window.__PERF__.longTasks.push({ start: e.startTime, ms: e.duration });
          }
        }).observe({ entryTypes: ["longtask"] });
      } catch {
        /* longtask unsupported — scenarios report null rather than zero */
      }
    },
    { responses, ipcDelayMs },
  );

  await page.goto(pathToFileURL(join(fx.UI, "index.html")).href);
  // app.js is `defer`red and init() is async; wait for the document to land
  // rather than racing it.
  await page.waitForFunction(
    () => document.getElementById("content")?.children.length > 0 || !window.__TAURI__,
    { timeout: 120_000 },
  );

  return { browser, page, docPath };
}

/** Emit a JSON result on stdout and close down. */
export async function finish(browser, result) {
  await browser.close();
  process.stdout.write(JSON.stringify(result, null, 2) + "\n");
}

export function stats(values) {
  if (!values.length) return null;
  const a = [...values].sort((x, y) => x - y);
  const at = (p) => a[Math.floor((a.length - 1) * p)];
  return {
    p50: at(0.5),
    p95: at(0.95),
    max: a[a.length - 1],
    mean: a.reduce((s, v) => s + v, 0) / a.length,
  };
}
