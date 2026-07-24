// Command-palette keystroke latency.
//
//   node scenarios/palette.mjs [--repo 500] [--query notes]
//
// `pin.oninput = () => runPalette(pin.value)` has no debounce, so every
// keystroke round-trips to `fuzzy_search` and then rebuilds up to 200 result
// rows through `innerHTML`. `movePalette` re-renders all 200 rows again on
// every arrow key, purely to move one `.sel` class.
//
// The IPC side is stubbed at zero latency here on purpose: this scenario
// isolates the *frontend's* per-keystroke cost. Backend query cost is measured
// properly by the `keystrokes` group in benches/search.rs.

import { launch, finish, stats } from "../tauri-stub.mjs";

const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : process.argv[i + 1];
};

const repoFiles = Number(arg("repo", 500));
const query = arg("query", "notes");

const { browser, page } = await launch({
  variant: "mixed",
  size: "128k",
  repoFiles,
  openInitial: true,
});

const result = await page.evaluate(
  async ({ query }) => {
    const overlay = document.getElementById("palette-overlay");
    const input = document.getElementById("palette-input");
    const box = document.getElementById("palette-results");

    window.openPalette();
    await new Promise((r) => requestAnimationFrame(r));

    // Type one character at a time, exactly as `oninput` fires. The rAF wait
    // is deliberately OUTSIDE the timer: in headless Chromium a frame is ~8ms,
    // which would swamp the work being measured and make every change look
    // like noise.
    const keystrokes = [];
    for (let i = 1; i <= query.length; i++) {
      input.value = query.slice(0, i);
      const t0 = performance.now();
      await window.runPalette(input.value);
      keystrokes.push(performance.now() - t0);
      await new Promise((r) => requestAnimationFrame(r));
    }

    const rows = box.children.length;

    // Arrow-key navigation: each press re-renders the whole result list just to
    // move one `.sel` class. Synchronous, so time it directly.
    const arrows = [];
    for (let i = 0; i < 20; i++) {
      const t0 = performance.now();
      window.movePalette(1);
      arrows.push(performance.now() - t0);
      await new Promise((r) => requestAnimationFrame(r));
    }

    const open = overlay.classList.contains("open");
    window.closePalette();
    return { keystrokes, arrows, rows, open };
  },
  { query },
);

await finish(browser, {
  source: "chromium",
  note: "relative regression detection only — Chromium is not WKWebView",
  scenario: "palette",
  repoFiles,
  query,
  renderedRows: result.rows,
  keystroke_ms: stats(result.keystrokes),
  arrowkey_ms: stats(result.arrows),
});
