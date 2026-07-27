// `node --test ui/paths.test.mjs`
//
// The frontend half of tenet 4: a markdown document may name any relative path
// it likes, and `normalizePath` + `insideRepo` are what decide whether the
// result is loaded or dropped. Both live in `ui/paths.js` as a classic script,
// so this evaluates that file in a fresh `vm` context and reads the functions
// back off it — no bundler, no Playwright, and nothing here can drift from what
// the webview actually loads.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import vm from "node:vm";

const here = dirname(fileURLToPath(import.meta.url));
const context = vm.createContext({});
vm.runInContext(readFileSync(join(here, "paths.js"), "utf8"), context, {
  filename: "paths.js",
});
const { normalizePath, insideRepo } = context;

test("paths.js defines both guards", () => {
  assert.equal(typeof normalizePath, "function");
  assert.equal(typeof insideRepo, "function");
});

// ---- normalizePath -------------------------------------------------------

test("normalizePath collapses . and .. segments", () => {
  assert.equal(normalizePath("/repo/docs/../a.md"), "/repo/a.md");
  assert.equal(normalizePath("/repo/./docs/a.md"), "/repo/docs/a.md");
  assert.equal(normalizePath("/repo/docs/deep/../../a.md"), "/repo/a.md");
});

test("normalizePath squashes empty segments", () => {
  assert.equal(normalizePath("/repo//docs///a.md"), "/repo/docs/a.md");
  assert.equal(normalizePath("/repo/docs/"), "/repo/docs");
});

test("normalizePath absorbs .. past the top rather than escaping", () => {
  assert.equal(normalizePath("/repo/../../../../etc/passwd"), "/etc/passwd");
  assert.equal(normalizePath("/.."), "/");
  assert.equal(normalizePath(""), "/");
});

// ---- insideRepo ----------------------------------------------------------

test("insideRepo accepts the root itself and its descendants", () => {
  assert.equal(insideRepo("/w/notes", "/w/notes"), true);
  assert.equal(insideRepo("/w/notes/a.md", "/w/notes"), true);
  assert.equal(insideRepo("/w/notes/deep/nest/a.md", "/w/notes"), true);
});

test("insideRepo rejects a sibling that merely shares a prefix", () => {
  // The whole reason the boundary has to be the separator: a plain
  // `startsWith` accepts every one of these.
  assert.equal(insideRepo("/w/notes-private/secret.md", "/w/notes"), false);
  assert.equal(insideRepo("/w/notesx", "/w/notes"), false);
  assert.equal(insideRepo("/w/notes.bak/a.md", "/w/notes"), false);
});

test("insideRepo rejects anything above or beside the root", () => {
  assert.equal(insideRepo("/w", "/w/notes"), false);
  assert.equal(insideRepo("/etc/passwd", "/w/notes"), false);
  assert.equal(insideRepo("/", "/w/notes"), false);
});

test("insideRepo tolerates a trailing slash on the root", () => {
  assert.equal(insideRepo("/w/notes/a.md", "/w/notes/"), true);
  assert.equal(insideRepo("/w/notes-private/x.md", "/w/notes/"), false);
});

test("with no repo open nothing is inside", () => {
  for (const root of ["", null, undefined]) {
    assert.equal(insideRepo("/w/notes/a.md", root), false);
  }
});

// ---- the two together, as interceptLinks uses them ------------------------

test("a document cannot escape the repo with ../ in a link", () => {
  const base = "/w/notes/docs/";
  const resolve = (href) => normalizePath(base + href);
  assert.equal(insideRepo(resolve("sibling.md"), "/w/notes"), true);
  assert.equal(insideRepo(resolve("../top.md"), "/w/notes"), true);
  assert.equal(insideRepo(resolve("../../outside.md"), "/w/notes"), false);
  assert.equal(insideRepo(resolve("../../../../etc/passwd"), "/w/notes"), false);
});
