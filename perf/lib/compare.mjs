#!/usr/bin/env node
// CLI wrapper around lib/report.mjs, called by run.sh.
//
//   node perf/lib/compare.mjs <result.json> <baseline.json> [verbose]
//
// Exit codes are the tier's exit code, so a regression is visible to whatever
// invoked the run without having to parse the table:
//   0  fine, or no baseline yet
//   1  something regressed past the fail threshold
//   0  warnings only — deliberately not a failure, since a 5% move on a noisy
//      laptop is not worth blocking on

import { readFileSync } from "node:fs";
import { diff, render, worstStatus } from "./report.mjs";

const [, , resultPath, baselinePath, verboseFlag] = process.argv;
if (!resultPath || !baselinePath) {
  console.error("usage: compare.mjs <result.json> <baseline.json> [verbose]");
  process.exit(2);
}

const current = JSON.parse(readFileSync(resultPath, "utf8"));
const result = diff(current, baselinePath);

console.log(render(result, { verbose: verboseFlag === "1" }));

process.exit(worstStatus(result) === "fail" ? 1 : 0);
