// Chrome DevTools Protocol tracing.
//
// Needed because the interesting part of scroll cost — style recalc, paint,
// raster, composite — does not happen in JavaScript and cannot be timed from
// inside the page. `performance.now()` around a `scrollTop` assignment measures
// approximately nothing. The renderer's own trace events are the only honest
// source.

const CATEGORIES = [
  "disabled-by-default-devtools.timeline",
  "disabled-by-default-devtools.timeline.frame",
  "blink.user_timing",
  "latencyInfo",
].join(",");

/** Run `fn` with the renderer traced; returns its value plus the trace events. */
export async function withTrace(page, fn) {
  const client = await page.context().newCDPSession(page);
  await client.send("Tracing.start", {
    categories: CATEGORIES,
    transferMode: "ReturnAsStream",
  });

  const result = await fn();

  const complete = new Promise((resolve) => client.once("Tracing.tracingComplete", resolve));
  await client.send("Tracing.end");
  const { stream } = await complete;

  let raw = "";
  for (;;) {
    const chunk = await client.send("IO.read", { handle: stream, size: 1 << 20 });
    raw += chunk.data;
    if (chunk.eof) break;
  }
  await client.send("IO.close", { handle: stream });
  await client.detach();

  const parsed = JSON.parse(raw);
  return { result, events: Array.isArray(parsed) ? parsed : parsed.traceEvents || [] };
}

/** Renderer work, grouped the way the rendering pipeline actually runs. */
const GROUPS = {
  style: ["UpdateLayoutTree", "ScheduleStyleRecalculation", "InvalidateLayout"],
  layout: ["Layout"],
  paint: ["Paint", "PrePaint", "PaintImage"],
  raster: ["RasterTask", "Rasterize"],
  composite: ["CompositeLayers", "Commit", "UpdateLayerTree", "LayerizeContent"],
  script: ["FunctionCall", "EvaluateScript", "V8.Execute", "TimerFire", "EventDispatch"],
  parse: ["ParseHTML", "ParseAuthorStyleSheet"],
};

/**
 * Total duration in ms per group, plus the heaviest individual event names.
 * Only complete ("X") events carry a duration; everything else is ignored.
 */
export function summarizeTrace(events) {
  const byName = new Map();
  for (const e of events) {
    if (e.ph !== "X" || typeof e.dur !== "number") continue;
    byName.set(e.name, (byName.get(e.name) || 0) + e.dur / 1000);
  }

  const groups = {};
  for (const [group, names] of Object.entries(GROUPS)) {
    groups[group + "_ms"] = Number(
      names.reduce((sum, n) => sum + (byName.get(n) || 0), 0).toFixed(2),
    );
  }
  groups.total_ms = Number(
    Object.entries(groups).reduce((s, [, v]) => s + v, 0).toFixed(2),
  );
  // `RunTask` wraps the other main-thread events, so it is deliberately kept
  // out of `total_ms` — summing it in would double-count. It is reported on its
  // own because it is the best single proxy for "how busy was the main thread".
  groups.main_thread_task_ms = Number((byName.get("RunTask") || 0).toFixed(2));

  const top = [...byName.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 8)
    .map(([name, ms]) => ({ name, ms: Number(ms.toFixed(2)) }));

  return { groups, top };
}
