// Highlight re-application cost.
//
//   node scenarios/highlight.mjs [--variant mixed] [--size 2m]
//
// `applyHighlights` (app.js) creates a **fresh TreeWalker over the entire
// content element for every highlight** and scans text nodes until it finds the
// quote. That's O(highlights x text nodes), restarting from the top of the
// document each time, and it runs on every render — including every re-render
// the file watcher triggers.
//
// It also only matches a quote living entirely inside one text node, so a
// highlight spanning inline markup (`code`, *em*, a link) silently fails and is
// demoted to a stale chip. Two quote sets are measured to separate cost from
// correctness:
//
//   single_node  quotes sampled from within one text node — the happy path.
//                Everything should anchor; this is the pure cost measurement.
//   spanning     quotes taken from an element's `textContent` across a child
//                boundary, which is exactly what `getSelection().toString()`
//                returns when the user drags over inline markup. These are
//                valid selections the user can make, and `staleRatio` reports
//                how many of them the current matcher drops on the floor.
//
// Quotes are sampled from the rendered DOM, not from the markdown source: the
// corpus fixtures still carry markdown syntax, which never appears in the DOM,
// so source-derived quotes would report a meaningless 100% failure.
//
// `ui/app.js` is a classic script, so its top-level functions are globals and
// can be driven directly.

import { launch, finish, stats } from "../tauri-stub.mjs";

const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : process.argv[i + 1];
};

const variant = arg("variant", "mixed");
const size = arg("size", "2m");
const COUNTS = [1, 10, 100, 500];

const { browser, page, docPath } = await launch({ variant, size, openInitial: true });

const results = { single_node: {}, spanning: {} };
for (const mode of ["single_node", "spanning"]) {
  for (const n of COUNTS) {
    results[mode][n] = await page.evaluate(
      async ({ n, mode, docPath, repeats }) => {
        const el = document.getElementById("content");
        const pristine = el.innerHTML;

        // Deterministic sampling: a fixed coprime stride, no randomness, so
        // repeat runs compare against each other.
        const sample = () => {
          const out = [];
          if (mode === "single_node") {
            const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
            const nodes = [];
            while (walker.nextNode()) {
              const t = walker.currentNode.nodeValue.trim();
              if (t.length >= 40) nodes.push(t);
            }
            for (let i = 0; out.length < n && i < nodes.length; i++) {
              out.push(nodes[(i * 7919) % nodes.length].slice(0, 60));
            }
          } else {
            // Elements with more than one child node: slicing their
            // `textContent` across the boundary reproduces what a user's
            // selection over inline markup actually yields.
            const els = [...el.querySelectorAll("p, li, td, h2, h3")].filter(
              (e) => e.childNodes.length > 1 && e.textContent.trim().length >= 60,
            );
            for (let i = 0; out.length < n && i < els.length; i++) {
              const e = els[(i * 7919) % els.length];
              const first = e.childNodes[0].textContent.length;
              const text = e.textContent;
              const start = Math.max(0, first - 20);
              out.push(text.slice(start, Math.min(text.length, first + 40)));
            }
          }
          return out;
        };

        const quotes = sample();
        const fixtures = quotes.map((q, i) => ({
          id: i + 1,
          file_path: docPath,
          line_start: 0,
          line_end: 0,
          quote: q,
          prefix: "",
          suffix: "",
          state: "active",
          annotation: null,
        }));

        const times = [];
        let applied = 0;
        for (let i = 0; i < repeats; i++) {
          // Re-applying onto an already-wrapped DOM would measure the wrong
          // thing, so reset to the unhighlighted document each round.
          el.innerHTML = pristine;
          void el.offsetHeight;

          const t0 = performance.now();
          window.applyHighlights(fixtures);
          times.push(performance.now() - t0);

          applied = el.querySelectorAll("mark.hl").length;
          await new Promise((r) => requestAnimationFrame(r));
        }

        el.innerHTML = pristine;
        return { times, applied, requested: fixtures.length };
      },
      { n, mode, docPath, repeats: 3 },
    );
  }
}

const shape = (r) => ({
  apply_ms: stats(r.times),
  requested: r.requested,
  applied: r.applied,
  // Fraction of valid selections the matcher failed to anchor.
  failRatio: r.requested ? Number(((r.requested - r.applied) / r.requested).toFixed(3)) : 0,
});

await finish(browser, {
  source: "chromium",
  note: "relative regression detection only — Chromium is not WKWebView",
  scenario: "highlight",
  variant,
  size,
  singleNode: Object.fromEntries(Object.entries(results.single_node).map(([n, r]) => [n, shape(r)])),
  spanning: Object.fromEntries(Object.entries(results.spanning).map(([n, r]) => [n, shape(r)])),
});
