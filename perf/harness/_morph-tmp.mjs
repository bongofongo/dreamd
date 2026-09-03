import { chromium } from "playwright";
import { readFileSync } from "node:fs";
const D = "/tmp/claude-1000/-home-oliver-Projects-dreamd/5c31b8b8-13a1-4fc4-9ef8-5818a19e0054/scratchpad";
const html = readFileSync(`${D}/mixed-2m.html`, "utf8");

const shell = `<!doctype html><html><head><style>
  body{margin:0;font:16px/1.6 system-ui;color:#dfe3e8;background:#14121c}
  #scroll{height:900px;overflow:auto}
  #content{max-width:700px;margin:0 auto;padding:32px 40px 120px}
  #content pre{overflow-x:auto;padding:12px;border-radius:6px}
</style></head><body><div id="scroll"><div id="content"></div></div></body></html>`;

const browser = await chromium.launch();
const REPS = 11;
const res = { innerHTML: [], patch1: [] };

for (let i = 0; i < REPS; i++) {
  const arms = ["innerHTML", "patch1"];
  if (i % 2) arms.reverse();
  for (const arm of arms) {
    const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
    await page.setContent(shell);
    const r = await page.evaluate(([h, mode]) => {
      const el = document.getElementById("content");
      // First render + full layout, exactly as a launch would do.
      el.innerHTML = h;
      void el.offsetHeight;

      // Simulate one paragraph edited in the middle of the document.
      const n = el.children.length;
      const idx = Math.floor(n / 2);
      const edited = h.replace(el.children[idx].outerHTML,
        el.children[idx].outerHTML.replace(/>/, ">EDITED "));

      const t0 = performance.now();
      if (mode === "innerHTML") {
        el.innerHTML = edited;                       // what dreamd does today
      } else {
        // Block-level patch: parse detached, swap the one child that differs.
        const tpl = document.createElement("template");
        tpl.innerHTML = edited;
        const fresh = tpl.content.children;
        for (let k = 0; k < fresh.length; k++) {
          if (fresh[k].outerHTML !== el.children[k].outerHTML) {
            el.replaceChild(fresh[k].cloneNode(true), el.children[k]);
            break;
          }
        }
      }
      const t1 = performance.now();
      void el.offsetHeight;                          // force style + layout
      const t2 = performance.now();
      return { blocks: n, mutate: t1 - t0, layout: t2 - t1, total: t2 - t0 };
    }, [html, arm]);
    res[arm].push(r);
    await page.close();
  }
}

const stat = (a, f) => { const s = a.map(f).sort((x, y) => x - y); return { min: s[0], med: s[Math.floor(s.length / 2)] }; };
console.log(`n=${REPS} per arm, fresh page each, order alternated`);
console.log(`document: ${res.innerHTML[0].blocks} top-level blocks, one edited\n`);
for (const k of ["mutate", "layout", "total"]) {
  const a = stat(res.innerHTML, (r) => r[k]);
  const b = stat(res.patch1, (r) => r[k]);
  const d = (x, y) => `${y - x > 0 ? "+" : ""}${(100 * (y - x) / x).toFixed(1)}%`;
  console.log(`${k.padEnd(7)} innerHTML min ${a.min.toFixed(1).padStart(7)}  patch min ${b.min.toFixed(1).padStart(7)}  ${d(a.min, b.min).padStart(8)}` +
              `    med ${a.med.toFixed(1).padStart(7)} -> ${b.med.toFixed(1).padStart(7)}  ${d(a.med, b.med).padStart(8)}`);
}
await browser.close();
