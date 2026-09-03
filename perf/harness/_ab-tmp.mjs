import { chromium } from "playwright";
import { readFileSync } from "node:fs";
const D = "/tmp/claude-1000/-home-oliver-Projects-dreamd/5c31b8b8-13a1-4fc4-9ef8-5818a19e0054/scratchpad";
const A = readFileSync(`${D}/mixed-2m.html`, "utf8");            // today: span per token
const B = readFileSync(`${D}/mixed-2m-nodefault.html`, "utf8");  // default-colour spans dropped

// Arm B is only correct if the <pre> carries the default foreground itself,
// since the dropped spans were what supplied it. That declaration is part of
// the change, so it is part of the arm.
const shell = (extra) => `<!doctype html><html><head><style>
  body{margin:0;font:16px/1.6 system-ui;color:#dfe3e8;background:#14121c}
  #scroll{height:900px;overflow:auto}
  #content{max-width:700px;margin:0 auto;padding:32px 40px 120px}
  #content pre{overflow-x:auto;padding:12px;border-radius:6px}
  ${extra}
</style></head><body><div id="scroll"><div id="content"></div></div></body></html>`;

const browser = await chromium.launch();
const REPS = 15;
const res = { today: [], nodefault: [] };

for (let i = 0; i < REPS; i++) {
  const pair = [["today", A, ""], ["nodefault", B, "#content pre{color:#c0c5ce}"]];
  if (i % 2) pair.reverse();
  for (const [name, html, extra] of pair) {
    const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
    await page.setContent(shell(extra));
    const r = await page.evaluate((h) => {
      const el = document.getElementById("content");
      const t0 = performance.now();
      el.innerHTML = h;
      const t1 = performance.now();
      void el.offsetHeight;
      const t2 = performance.now();
      // Scroll cost: the interaction a reader spends all their time in.
      const sc = document.getElementById("scroll");
      const t3 = performance.now();
      for (let k = 1; k <= 30; k++) { sc.scrollTop = k * 900; void sc.offsetHeight; }
      const t4 = performance.now();
      return { parse: t1 - t0, layout: t2 - t1, scroll: t4 - t3 };
    }, html);
    res[name].push(r);
    await page.close();
  }
}

const stat = (a, f) => { const s = a.map(f).sort((x, y) => x - y); return { min: s[0], med: s[Math.floor(s.length / 2)] }; };
console.log(`n=${REPS} per arm, fresh page each, order alternated\n`);
for (const k of ["parse", "layout", "scroll"]) {
  const i = stat(res.today, (r) => r[k]);
  const c = stat(res.nodefault, (r) => r[k]);
  const d = (a, b) => `${b - a > 0 ? "+" : ""}${(100 * (b - a) / a).toFixed(1)}%`;
  console.log(`${k.padEnd(7)} min: ${i.min.toFixed(1).padStart(7)} -> ${c.min.toFixed(1).padStart(7)}  ${d(i.min, c.min).padStart(7)}` +
              `   med: ${i.med.toFixed(1).padStart(7)} -> ${c.med.toFixed(1).padStart(7)}  ${d(i.med, c.med).padStart(7)}`);
}
await browser.close();
