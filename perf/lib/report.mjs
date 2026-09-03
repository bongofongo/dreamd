// Flattens a tier result into comparable metrics and diffs it against a
// baseline. run.sh resolves where that is; this file only takes the path.
//
// A metric is one dot-path with a number at the end. Keeping the comparison
// dumb and path-based means adding a measurement anywhere in any script shows
// up in the table automatically, with no registry to keep in sync.

import { existsSync, readFileSync } from "node:fs";

/** Metrics where a *larger* number is better. Everything else: lower is better. */
const HIGHER_IS_BETTER = [/\bapplied$/, /\bthroughput/];

/**
 * Per-source sensitivity, set from the measured noise floor rather than taste.
 *
 * Two consecutive runs on identical code were compared to find these. Rust
 * benches did not move — criterion's sampling does its job, and every tier now
 * uses identical criterion settings so their numbers are directly comparable.
 * The Chromium scenarios drifted up to 17% on raster and composite from run to
 * run with nothing changed, because they are single-sample measurements of a
 * whole browser engine.
 *
 * Thresholds below the noise floor don't catch more regressions, they just
 * produce noise that trains you to ignore the tool.
 */
const THRESHOLDS = [
  { match: /^chromium\./, warn: 0.2, fail: 0.35 },
  // The save loop's frontend-await metrics, at the spread they actually have.
  //
  // These were briefly given a 100%/200% threshold, and the measurement behind
  // that was real: five `loop.sh --release --highlights 100 --saves 12` runs on
  // identical code gave p50s of save_to_paint 1669-3369ms and
  // ipc_render_markdown 177-1307ms. They were bimodal because each times an
  // `await` and whichever await yielded first also absorbed the webview
  // re-laying out the whole document, so the cost teleported between metrics.
  //
  // `writeContent`'s block patching removed that layout, and with it the
  // bimodality. Five runs now: save_to_paint 382-390ms, ipc_render_markdown
  // 111-114ms, apply_highlights 60-63ms, ipc_reanchor 64-70ms — the widest of
  // them ±9%, against ±102% before. So the threshold comes back down; leaving it
  // at 100% after fixing the cause would hide a genuine regression behind a
  // workaround for a problem that no longer exists.
  //
  // Still looser than the default 5%/15%, because p50 here is taken over the 12
  // repaints one run produces and `ipc_reanchor` sits at ±9% of a small number.
  {
    match: /^real\.loop\..*\.(save_to_paint_ms|ipc_reanchor_ms|ipc_render_markdown_ms|apply_highlights_ms)\./,
    warn: 0.15,
    fail: 0.3,
  },
  { match: /./, warn: 0.05, fail: 0.15 },
];

function thresholdFor(path) {
  return THRESHOLDS.find((t) => t.match.test(path));
}

/** Paths that are descriptive, not performance — never flagged as regressions. */
const IGNORE = [
  /^meta\./,
  // `max` is the single noisiest statistic in every sample set — it drifted 25%
  // run-to-run on unchanged code — and says the least. p50/p95 carry the signal.
  /\.max$/,
  /\.note$/,
  /\.source$/,
  /symbolicated$/,
  /^profile\.instruments_traces/,
  /^profile\.samply/,
  /^profile\.sample_fallback/,
  /requested$/,
  /\.steps$/,
  /\.stepPx$/,
  /\.wheels$/,
  /documentPx$/,
  /totalScrollPx$/,
  /\brows$/,
  /renderedRows$/,
  /\bsaves$/,
  /\bhighlights$/,
  /repoFiles$/,
  // Raw counts that scale with the tier's own parameters — `pass` drives 8 saves
  // and `deep` drives 12, so comparing one against the other would flag these
  // every time. `events_per_save` is the normalized figure that carries the
  // actual signal.
  //
  // Anchored past the workload key rather than straight after `loop.`. These
  // read `/loop\.watcher_events$/` and `/loop\.repaints$/`, which stopped
  // matching anything the moment real-app metrics were keyed by build profile
  // and highlight count: the path is `real.loop.release-h100.repaints`, so both
  // rules were dead while the paragraph above still claimed they were ignored,
  // and both counts were flagged as regressions every run.
  /^real\.loop\..*\.watcher_events$/,
  /^real\.loop\..*\.repaints$/,
  /scrolledPx$/,
];

export function flatten(obj, prefix = "", out = {}) {
  for (const [k, v] of Object.entries(obj ?? {})) {
    const path = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === "object" && !Array.isArray(v)) flatten(v, path, out);
    else if (typeof v === "number" && Number.isFinite(v)) out[path] = v;
  }
  return out;
}

const ignored = (path) => IGNORE.some((re) => re.test(path));
const higherBetter = (path) => HIGHER_IS_BETTER.some((re) => re.test(path));

/**
 * A percentage is meaningless below the clock's useful resolution.
 *
 * Every `real.*` leaf is milliseconds, and several of them are microseconds in
 * practice: `d:rust_get_highlights` is 0.0015ms, `process_start` 0.002ms,
 * `d:marks_loaded` 0.01ms. Against a 5% threshold those flag constantly —
 * 0.00150 -> 0.00180 was duly printed as a 20% regression, which is 300
 * nanoseconds of scheduler jitter wearing a regression's clothes, and it is
 * exactly the kind of red row that teaches you to stop reading the table.
 *
 * So a row where *both* sides sit under a millisecond is reported with its
 * numbers but never flagged. The exception is the handful of `real.*` leaves
 * that are ratios rather than durations: `events_per_save` lives around 1.0 and
 * a move from 1 to 2 means every save is rendering the document twice, which is
 * the single most valuable thing this loop can tell you.
 */
const RATIO_NOT_MS = /events_per_save$/;
const subMs = (path, before, after) =>
  /^real\./.test(path) &&
  !RATIO_NOT_MS.test(path) &&
  Math.abs(before) < 1 &&
  Math.abs(after) < 1;

/**
 * Thresholds are per-path and come from THRESHOLDS above, not from an
 * argument — there is no one sensitivity that suits a criterion mean and a
 * single-sample browser measurement at once.
 *
 * @param {object} current   this run's result document
 * @param {string} baselinePath
 */
export function diff(current, baselinePath) {
  const now = flatten(current);

  if (!existsSync(baselinePath)) {
    // Carry the path out: render() has to name it, and there is no fixed
    // answer to name any more — run.sh resolves it from three candidates.
    return {
      rows: [],
      missingBaseline: true,
      baselinePath,
      metrics: Object.keys(now).length,
    };
  }
  const base = flatten(JSON.parse(readFileSync(baselinePath, "utf8")));

  const rows = [];
  for (const [path, value] of Object.entries(now)) {
    if (ignored(path)) continue;
    const before = base[path];
    if (before === undefined) {
      rows.push({ path, before: null, after: value, delta: null, status: "new" });
      continue;
    }
    // A metric that was zero can't produce a meaningful ratio; report the
    // absolute move instead of dividing by zero and printing Infinity.
    let delta = null;
    if (before !== 0) delta = (value - before) / before;
    const worse = higherBetter(path) ? -(delta ?? 0) : (delta ?? 0);
    const { warn, fail } = thresholdFor(path);

    let status = "ok";
    if (delta === null) status = value === 0 ? "ok" : "new";
    else if (subMs(path, before, value)) status = "ok";
    else if (worse >= fail) status = "fail";
    else if (worse >= warn) status = "warn";
    else if (worse <= -warn) status = "better";

    rows.push({ path, before, after: value, delta, status });
  }

  // Anything the baseline has that this run didn't produce — usually a skipped
  // tier, occasionally a script that silently failed. Worth surfacing.
  const missing = Object.keys(base).filter((p) => !(p in now) && !ignored(p));

  return { rows, missing, missingBaseline: false };
}

const ICON = { ok: "  ", warn: "! ", fail: "XX", better: "+ ", new: "* " };

function fmt(n) {
  if (n === null || n === undefined) return "-";
  if (Number.isInteger(n)) return String(n);
  if (Math.abs(n) >= 1000) return n.toFixed(0);
  if (Math.abs(n) >= 1) return n.toFixed(2);
  return n.toPrecision(3);
}

/** Human-readable table. Chromium-sourced rows are grouped separately. */
export function render(result, { verbose = false } = {}) {
  if (result.missingBaseline) {
    return [
      `no baseline at ${result.baselinePath} — recorded ${result.metrics} metrics.`,
      `establish one with:  ./perf/run.sh deep --update-baseline`,
    ].join("\n");
  }

  const interesting = result.rows.filter((r) => r.status !== "ok");
  const shown = verbose ? result.rows : interesting;

  const lines = [];
  const bySource = { bench: [], "real-app": [], chromium: [], other: [] };
  for (const r of shown) {
    const key = r.path.startsWith("chromium.")
      ? "chromium"
      : r.path.startsWith("real.")
        ? "real-app"
        : r.path.startsWith("bench.")
          ? "bench"
          : "other";
    bySource[key].push(r);
  }

  const section = (title, rows, caveat) => {
    if (!rows.length) return;
    lines.push("", title);
    if (caveat) lines.push(`  (${caveat})`);
    const w = Math.max(...rows.map((r) => r.path.length), 10);
    for (const r of rows) {
      const d =
        r.delta === null ? "" : `${r.delta >= 0 ? "+" : ""}${(r.delta * 100).toFixed(1)}%`;
      lines.push(
        `  ${ICON[r.status]} ${r.path.padEnd(w)}  ${fmt(r.before).padStart(10)} -> ${fmt(r.after).padStart(10)}  ${d}`,
      );
    }
  };

  section("RUST BENCHES", bySource.bench, "native, criterion means in ms");
  section("REAL APP", bySource["real-app"]);
  section("CHROMIUM HARNESS", bySource.chromium, "relative only; not WKWebView timings");
  section("OTHER", bySource.other);

  if (result.missing?.length) {
    lines.push("", `not measured this run (${result.missing.length}):`);
    for (const p of result.missing.slice(0, 12)) lines.push(`    ${p}`);
    if (result.missing.length > 12) lines.push(`    ... and ${result.missing.length - 12} more`);
  }

  const fails = result.rows.filter((r) => r.status === "fail").length;
  const warns = result.rows.filter((r) => r.status === "warn").length;
  const betters = result.rows.filter((r) => r.status === "better").length;
  lines.push(
    "",
    `${result.rows.length} metrics compared: ${fails} regressed, ${warns} slower, ${betters} improved.`,
  );
  if (!interesting.length) lines.push("nothing moved beyond +/-5%.");

  return lines.join("\n");
}

export function worstStatus(result) {
  if (result.missingBaseline) return "no-baseline";
  if (result.rows.some((r) => r.status === "fail")) return "fail";
  if (result.rows.some((r) => r.status === "warn")) return "warn";
  return "ok";
}
