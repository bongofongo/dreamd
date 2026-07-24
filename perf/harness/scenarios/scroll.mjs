// Scroll cost — the stutter number.
//
//   node scenarios/scroll.mjs [--variant mixed] [--size 2m] [--highlights 0]
//
// Runs twice: once normally, once with `body.hl-mode` set. Highlight mode adds
// `box-shadow: inset 0 3px 0` to `#content-scroll` (index.html:94) — an inset
// shadow on the scrolling container itself, which can force the scroll area to
// repaint every frame. Comparing the two passes isolates that.
//
// HOW THIS IS MEASURED
//
// Not with `performance.now()`. Style recalc, paint, raster and composite all
// happen outside JavaScript, so timing a `scrollTop` assignment from inside the
// page reports ~0 no matter how expensive the scroll actually is. Instead the
// scroll is driven through the real input pipeline (`mouse.wheel`) with the
// renderer traced via CDP, and the engine's own timeline events are summed.
//
// Chromium is still not WKWebService — treat these as relative numbers for
// detecting regressions and ranking fixes, not as the app's true timings.
// `paint_ms` and `raster_ms` are what `content-visibility: auto` (fix B12) is
// meant to cut.

import { launch, finish, stats } from "../tauri-stub.mjs";
import { withTrace, summarizeTrace } from "../lib/trace.mjs";

const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : process.argv[i + 1];
};

const variant = arg("variant", "mixed");
const size = arg("size", "2m");
const highlights = Number(arg("highlights", 0));
const WHEELS = Number(arg("wheels", 60));

const { browser, page } = await launch({ variant, size, highlights });

async function pass(hlMode) {
  await page.evaluate((on) => {
    document.body.classList.toggle("hl-mode", on);
    document.getElementById("content-scroll").scrollTop = 0;
    window.__PERF__.longTasks.length = 0;
  }, hlMode);

  // Settle before tracing so the class change isn't attributed to the scroll.
  await page.waitForTimeout(300);
  await page.mouse.move(600, 500);

  const { result, events } = await withTrace(page, async () => {
    const t0 = Date.now();
    for (let i = 0; i < WHEELS; i++) {
      await page.mouse.wheel(0, 600);
      // Roughly one wheel tick per frame; enough for the compositor to keep up
      // without the loop outrunning it entirely.
      await page.waitForTimeout(16);
    }
    return { wall_ms: Date.now() - t0 };
  });

  const scrolled = await page.evaluate(() => {
    const s = document.getElementById("content-scroll");
    return { scrollTop: s.scrollTop, scrollHeight: s.scrollHeight, client: s.clientHeight };
  });
  const longTasks = await page.evaluate(() => [...window.__PERF__.longTasks]);

  const { groups, top } = summarizeTrace(events);
  return {
    hlMode,
    wheels: WHEELS,
    wall_ms: result.wall_ms,
    scrolledPx: scrolled.scrollTop,
    documentPx: scrolled.scrollHeight,
    renderer: groups,
    heaviest: top,
    longTasks: longTasks.length,
    longTask_ms: stats(longTasks.map((t) => t.ms)),
  };
}

const normal = await pass(false);
const highlightMode = await pass(true);

await finish(browser, {
  source: "chromium",
  note: "relative regression detection only — Chromium is not WKWebView; renderer_ms come from CDP trace events",
  scenario: "scroll",
  variant,
  size,
  highlights,
  // Top-level so the runner can key results by it. Wheel count scales every
  // total in here, so two runs with different counts must not share a path.
  wheels: WHEELS,
  normal,
  highlightMode,
});
