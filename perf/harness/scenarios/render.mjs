// Document render cost in the frontend: the `innerHTML` swap and everything
// `renderCurrent` does after it.
//
//   node scenarios/render.mjs [--variant mixed] [--size 2m] [--repeats 5]
//
// `ui/app.js:130` replaces the entire document in one assignment, with no
// virtualization and no chunking, then walks it twice more in
// `interceptLinks()`. That is the cost this measures.

import { launch, finish, stats } from "../tauri-stub.mjs";

const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : process.argv[i + 1];
};

const variant = arg("variant", "mixed");
const size = arg("size", "2m");
const repeats = Number(arg("repeats", 5));

const { browser, page } = await launch({ variant, size, openInitial: true });

const html = await page.evaluate(() => document.getElementById("content").innerHTML);

const samples = await page.evaluate(
  async ({ html, repeats }) => {
    const el = document.getElementById("content");
    const swap = [];
    const layout = [];

    for (let i = 0; i < repeats; i++) {
      el.innerHTML = "";
      // Force the empty state to settle so the next measurement starts clean.
      void el.offsetHeight;

      const t0 = performance.now();
      el.innerHTML = html;
      const t1 = performance.now();
      // Reading offsetHeight forces style + layout to actually run; without it
      // the innerHTML number is just parse cost and the real work lands later
      // in a frame we never see.
      void el.offsetHeight;
      const t2 = performance.now();

      swap.push(t1 - t0);
      layout.push(t2 - t1);
      await new Promise((r) => requestAnimationFrame(r));
    }

    return {
      swap,
      layout,
      htmlBytes: html.length,
      nodes: el.querySelectorAll("*").length,
      links: el.querySelectorAll("a[href]").length,
      images: el.querySelectorAll("img[src]").length,
      codeBlocks: el.querySelectorAll("pre").length,
      // syntect emits one span per token; this is the payload-inflation number
      // that fix B9 (ClassedHTMLGenerator) is meant to cut.
      inlineStyledSpans: el.querySelectorAll("span[style]").length,
    };
  },
  { html, repeats },
);

await finish(browser, {
  source: "chromium",
  note: "relative regression detection only — Chromium is not WKWebView",
  scenario: "render",
  variant,
  size,
  htmlBytes: samples.htmlBytes,
  dom: {
    nodes: samples.nodes,
    links: samples.links,
    images: samples.images,
    codeBlocks: samples.codeBlocks,
    inlineStyledSpans: samples.inlineStyledSpans,
  },
  innerhtml_ms: stats(samples.swap),
  forced_layout_ms: stats(samples.layout),
});
