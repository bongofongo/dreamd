// Functional check for the settings panel. Loads the real `ui/` in Chromium
// behind a `window.__TAURI__` stub that behaves like the Rust commands, drives
// the panel, and asserts on the DOM. Exits non-zero on any failure.
//
//   node perf/harness/ui-check.mjs
//
// This is the frontend's only correctness harness, and it lives here because
// this is the one place with Playwright installed. It is NOT part of any perf
// tier — no timings are taken and nothing it does feeds a baseline.
//
// Chromium is not WKWebView, but nothing asserted here is engine-specific: it
// is DOM structure, event wiring, and which IPC calls the panel makes. The
// same caveat as the perf scenarios applies to anything visual.
import { chromium } from "playwright";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { readFileSync, readdirSync } from "node:fs";

// perf/harness/ -> repo root -> ui/. Derived, not hardcoded: this was an
// absolute path into one laptop's checkout, so the harness ran nowhere else —
// including CI, which is why nothing caught it.
const UI = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "ui");
const base = readFileSync(join(UI, "theme.css"), "utf8");
const palettes = Object.fromEntries(
  readdirSync(join(UI, "themes")).map((f) => [
    f.replace(/\.css$/, ""),
    readFileSync(join(UI, "themes", f), "utf8"),
  ]),
);

const results = [];
const check = (what, cond, extra = "") =>
  results.push(`${cond ? "ok  " : "FAIL"} ${what}${cond ? "" : "  " + extra}`);

const browser = await chromium.launch();
// Chromium defaults to `prefers-color-scheme: light`, and app.js reads that
// before anything paints — so without pinning it here every hardcoded hex below
// would be asserting against a palette's *light* block. Pinned rather than
// merely accounted for: the appearance the assertions run in should be a
// decision, not a browser default that can change.
const newPage = () =>
  browser.newPage({ viewport: { width: 1280, height: 900 }, colorScheme: "dark" });
const page = await newPage();
page.on("pageerror", (e) => results.push("FAIL pageerror: " + e.message));
page.on("console", (m) => {
  if (m.type() === "error") results.push("FAIL console.error: " + m.text());
});

await page.addInitScript(({ base, palettes }) => {
  const KEYMAP = {
    palette: "Ctrl+F", palette_prev: "Ctrl+P", palette_next: "Ctrl+N",
    highlight: "Ctrl+H", send_stack: "Ctrl+Enter", toggle_stack: "Ctrl+O",
    toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B", toggle_view: "Ctrl+M",
    jump_top: "Home", jump_bottom: "End", set_mark: "m", jump_mark: "'",
    jump_back: "Ctrl+[", jump_forward: "Ctrl+]",
    find: "/", find_next: "n", find_prev: "Shift+N",
    next_file: "]", prev_file: "[",
    copy_stack: "Ctrl+C", settings: "Ctrl+,",
    save_annotation: "Ctrl+Y",
    quick_highlight: true,
  };
  const state = {
    config: {
      theme: "dreamd", mode: "system",
      tmux_autodetect: true, extra_ignores: [], keymap: { ...KEYMAP },
    },
    // What `mode: "system"` resolves to. The page is opened with
    // `colorScheme: "dark"`, so this is what Rust would have answered.
    scheme: "dark",
    userThemes: {},
  };
  window.__STATE__ = state;

  const themeList = () => [
    ...Object.keys(state.userThemes).map((name) => ({ name, kind: "user", path: "/u/" + name })),
    ...Object.keys(palettes)
      .filter((n) => !(n in state.userThemes))
      .map((name) => ({ name, kind: "bundled", path: null })),
  ].sort((a, b) => a.name.localeCompare(b.name));

  const cssFor = (name) =>
    name in state.userThemes ? base + "\n" + state.userThemes[name]
      : name in palettes ? base + "\n" + palettes[name]
      : null;

  const themeView = () => ({
    css: cssFor(state.config.theme),
    mode: state.config.mode,
    scheme: state.config.mode === "system" ? state.scheme : state.config.mode,
    syntax_theme: null,
  });

  const settings = () => ({
    config: JSON.parse(JSON.stringify(state.config)),
    theme: state.config.theme,
    scheme: themeView().scheme,
    themes: themeList(),
    syntax_themes: ["base16-ocean.dark", "InspiredGitHub", "Solarized (light)"],
    config_path: "/tmp/xdg/dreamd/config.toml",
    themes_dir: "/tmp/xdg/dreamd/themes",
    local_overrides: ["keymap.copy_stack"], // exercise the shadowed badge
  });

  const merge = (dst, src) => {
    for (const [k, v] of Object.entries(src)) {
      if (v && typeof v === "object" && !Array.isArray(v)) merge((dst[k] ||= {}), v);
      else dst[k] = v;
    }
  };

  window.__TAURI__ = {
    core: {
      async invoke(cmd, args) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return state.config.keymap;
          case "get_theme": return themeView();
          case "set_appearance":
            state.scheme = args.scheme;
            return themeView();
          case "list_markdown_files": return { name: "repo", is_dir: true, path: "/repo", children: [] };
          case "initial_file": return null;
          case "get_highlights": case "reanchor": case "get_stack": return [];
          case "get_settings": return settings();
          case "set_config":
            merge(state.config, args.patch);
            return settings();
          case "default_keymap": return { ...KEYMAP };
          case "list_themes": return themeList();
          case "theme_css": {
            const css = cssFor(args.name);
            if (!css) throw new Error("no theme named " + args.name);
            return css;
          }
          case "palette_css": {
            const css = args.name in state.userThemes
              ? state.userThemes[args.name] : palettes[args.name];
            if (!css) throw new Error("no theme named " + args.name);
            return css;
          }
          case "save_theme":
            state.userThemes[args.name] = args.css;
            return "/tmp/xdg/dreamd/themes/" + args.name + ".css";
          case "delete_theme":
            delete state.userThemes[args.name];
            return null;
          case "render_markdown": return "<h1>doc</h1>";
          default: return null;
        }
      },
    },
    event: { async listen() { return () => {}; } },
  };
}, { base, palettes });

await page.goto(pathToFileURL(join(UI, "index.html")).href);
await page.waitForFunction(() => document.getElementById("settings-overlay") !== null);
await page.waitForTimeout(300);

const varOf = (name) =>
  page.evaluate((n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim(), name);

// --- sidebar default, directory branch ---
// The markup ships `nav-collapsed` and `init()` removes it when `initial_file`
// is null — which is what this page's stub returns. The single-file branch,
// where the class survives, is checked on a second page at the end.
check(
  "no initial file leaves the sidebar open",
  !(await page.evaluate(() => document.body.classList.contains("nav-collapsed"))),
);

// --- the theme actually applies (this is the CSS split working) ---
check("base+palette applied on boot", (await varOf("--bg")) === "#14121c", await varOf("--bg"));
check(
  "content width comes from the palette",
  (await page.evaluate(() => getComputedStyle(document.getElementById("content")).maxWidth)) === "700px",
);

// --- open via the keybind ---
await page.keyboard.press("Control+Comma");
check("Ctrl+, opens settings", await page.locator("#settings-overlay.open").isVisible());

// --- keys tab ---
const rows = await page.locator("#st-keys .st-row").count();
check("every action gets a row", rows === 24, `got ${rows}`);
check(
  "a repo-shadowed key is flagged",
  (await page.locator("#st-keys .shadowed").count()) === 1,
);
const firstCombo = page.locator("#st-keys .st-row").first().locator("button.combo");
const shown = (await firstCombo.textContent()).trim();
check("combo is rendered", shown === "\u2303F" || shown === "Ctrl+F", shown);

// record a new binding for the palette
await firstCombo.click();
check("recording state shown", (await firstCombo.textContent()).includes("Press keys"));
await page.keyboard.press("Control+Shift+P");
await page.waitForTimeout(120);
check(
  "rebind persisted to the stub config",
  (await page.evaluate(() => window.__STATE__.config.keymap.palette)) === "Ctrl+Shift+P",
  await page.evaluate(() => window.__STATE__.config.keymap.palette),
);
check(
  "rebind is live in the frontend keymap",
  (await page.evaluate(() => window.keymap?.palette ?? "n/a")) !== "Ctrl+F",
);

// escape cancels a recording rather than closing the panel
await firstCombo.click();
await page.keyboard.press("Escape");
await page.waitForTimeout(80);
check("Esc cancels recording, panel stays open", await page.locator("#settings-overlay.open").isVisible());

// --- themes tab ---
await page.locator('.st-tab[data-pane="themes"]').click();
await page.waitForTimeout(400);
const cards = await page.locator("#st-theme-grid .th-card").count();
check("all palettes listed", cards === Object.keys(palettes).length, `got ${cards}`);
check(
  "swatches painted from the palette",
  (await page.locator("#st-theme-grid .th-card").first().locator(".th-swatch i").count()) === 4,
);

const nord = page.locator("#st-theme-grid .th-card", { hasText: "nord" }).first();
await nord.click();
await page.waitForTimeout(120);
check("clicking a card previews it", (await varOf("--bg")) === "#2e3440", await varOf("--bg"));

await nord.locator("button", { hasText: "Apply" }).click();
await page.waitForTimeout(250);
check(
  "apply persists the theme",
  (await page.evaluate(() => window.__STATE__.config.theme)) === "nord",
);
check(
  "preview cleared after applying",
  (await page.evaluate(() => document.getElementById("theme-preview").textContent)) === "",
);
check("applied theme is still painted", (await varOf("--bg")) === "#2e3440", await varOf("--bg"));

// --- custom tab ---
await page.locator("#st-theme-grid .th-card", { hasText: "gruvbox" }).first()
  .locator("button", { hasText: "Duplicate" }).click();
await page.waitForTimeout(250);
check("duplicate switches to the custom tab", await page.locator("#st-pane-custom.sel").isVisible());
// Six shared type metrics plus the fifteen in the appearance being edited
// (thirteen colours, --syntax-theme, --stale-text). An exact count on purpose:
// it is what catches the Custom tab silently listing one block instead of two.
const varRows = await page.locator("#st-vars .st-var").count();
check("var editor lists shared + one appearance", varRows === 21, `got ${varRows}`);
check(
  "colour vars get a picker",
  (await page.locator('#st-vars input[type="color"]').count()) >= 13,
);
check("syntax theme gets a select", (await page.locator("#st-vars select").count()) === 1);
check("duplicating previews the copy", (await varOf("--bg")) === "#282828", await varOf("--bg"));

// edit a colour and confirm it previews live
await page.evaluate(() => {
  const picker = document.querySelector('#st-vars input[type="color"]');
  picker.value = "#123456";
  picker.dispatchEvent(new Event("input", { bubbles: true }));
});
await page.waitForTimeout(120);
check("editing a var previews live", (await varOf("--bg")) === "#123456", await varOf("--bg"));
check(
  "the CSS textarea tracks the edit",
  (await page.locator("#st-custom-css").inputValue()).includes("#123456"),
);

// save it
await page.locator("#st-save-name").fill("my-theme");
await page.locator("#st-save-theme").click();
await page.waitForTimeout(300);
check(
  "saved as a user theme",
  (await page.evaluate(() => Object.keys(window.__STATE__.userThemes))).includes("my-theme"),
);
check(
  "and switched to",
  (await page.evaluate(() => window.__STATE__.config.theme)) === "my-theme",
);
check("saved theme is painted", (await varOf("--bg")) === "#123456", await varOf("--bg"));
await page.waitForTimeout(200);
check(
  "user theme shows in the list",
  (await page.locator("#st-theme-grid .th-card", { hasText: "my-theme" }).count()) === 1,
);

// --- appearance toggle ---
// Independent of which family is selected, which is the point of it. The theme
// here is still `my-theme`, a flat single-`:root` copy — so this also pins the
// backwards-compatibility rule: a palette with no mode blocks paints the same
// in both appearances rather than falling back to nothing.
await page.locator('.st-tab[data-pane="themes"]').click();
await page.waitForTimeout(150);
const modeBtns = await page.locator("#st-mode .st-mode-btn").count();
check("appearance offers light, dark and system", modeBtns === 3, `got ${modeBtns}`);

await page.locator("#st-mode .st-mode-btn", { hasText: "light" }).first().click();
await page.waitForTimeout(250);
check(
  "the toggle persists the mode",
  (await page.evaluate(() => window.__STATE__.config.mode)) === "light",
);
check(
  "and flips data-mode",
  (await page.evaluate(() => document.documentElement.dataset.mode)) === "light",
);

// Back to a family, in light, and assert it paints its *light* block — the
// whole reason mode blocks exist.
await page.locator("#st-theme-grid .th-card", { hasText: "gruvbox" }).first()
  .locator("button", { hasText: "Apply" }).click();
await page.waitForTimeout(250);
check("a family paints its light block", (await varOf("--bg")) === "#fbf1c7", await varOf("--bg"));
await page.locator("#st-mode .st-mode-btn", { hasText: "dark" }).first().click();
await page.waitForTimeout(250);
check("and its dark block", (await varOf("--bg")) === "#282828", await varOf("--bg"));

// The Custom tab edits one block at a time, so the var list is the shared block
// plus the appearance being edited — six and fourteen, which is why the count
// above is still twenty.
await page.locator("#st-theme-grid .th-card", { hasText: "gruvbox" }).first()
  .locator("button", { hasText: "Duplicate" }).click();
await page.waitForTimeout(250);
const blockBtns = await page.locator("#st-block .st-mode-btn").count();
check("custom tab offers both blocks", blockBtns === 2, `got ${blockBtns}`);
const darkRows = await page.locator("#st-vars .st-var").count();
check("editing dark lists shared + dark", darkRows === 21, `got ${darkRows}`);
await page.locator("#st-block .st-mode-btn", { hasText: "light" }).first().click();
await page.waitForTimeout(150);
const lightVal = await page.locator("#st-vars .st-var", { hasText: "--bg" })
  .first().locator('input[type="text"]').inputValue();
check("switching block shows the other appearance", lightVal === "#fbf1c7", lightVal);

// --- closing ---
await page.keyboard.press("Escape");
check("Esc closes the panel", !(await page.locator("#settings-overlay.open").isVisible()));

// --- sidebar default, single-file branch ---
// A second page, because the boot decision is made once per load. The tree
// resolves late on purpose: that is the deferred walk, and the point is that
// the document is on screen before it lands.
const solo = await newPage();
solo.on("pageerror", (e) => results.push("FAIL pageerror (single-file): " + e.message));
solo.on("console", (m) => {
  if (m.type() === "error") results.push("FAIL console.error (single-file): " + m.text());
});
await solo.addInitScript(({ base }) => {
  const tree = {
    name: "repo", is_dir: true, path: "/repo", rel: "",
    children: [{ name: "doc.md", is_dir: false, path: "/repo/doc.md", rel: "doc.md", children: [] }],
  };
  window.__TAURI__ = {
    core: {
      async invoke(cmd) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return {
            palette: "Ctrl+F", palette_prev: "Ctrl+P", palette_next: "Ctrl+N",
            highlight: "Ctrl+H", send_stack: "Ctrl+Enter", toggle_stack: "Ctrl+O",
            toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B", toggle_view: "Ctrl+M",
            jump_top: "Home", jump_bottom: "End", set_mark: "m", jump_mark: "'",
            jump_back: "Ctrl+[", jump_forward: "Ctrl+]",
            find: "/", find_next: "n", find_prev: "Shift+N",
            next_file: "]", prev_file: "[",
            copy_stack: "Ctrl+C", settings: "Ctrl+,",
            save_annotation: "Ctrl+Y",
            quick_highlight: true,
          };
          case "get_theme": return { css: base, mode: "system", scheme: "dark", syntax_theme: null };
          case "initial_file": return "/repo/doc.md";
          case "render_markdown": return "<h1>doc</h1>";
          // The deferred walk: resolves well after the document renders.
          case "list_markdown_files":
            return new Promise((r) => setTimeout(() => r(tree), 400));
          case "get_highlights": case "reanchor": case "get_stack": return [];
          default: return null;
        }
      },
    },
    event: { async listen() { return () => {}; } },
  };
}, { base });
await solo.goto(pathToFileURL(join(UI, "index.html")).href);
await solo.waitForFunction(() => document.getElementById("content").innerHTML.includes("doc"));
check(
  "an initial file keeps the sidebar collapsed",
  await solo.evaluate(() => document.body.classList.contains("nav-collapsed")),
);
check(
  "the document paints before the tree lands",
  (await solo.locator("#tree .tree-item").count()) === 0,
);
await solo.waitForSelector("#tree .tree-item", { timeout: 5000 });
check("the tree populates when the walk finishes", (await solo.locator("#tree .tree-item").count()) === 1);
check(
  "and the open file is marked active in it",
  (await solo.locator("#tree .tree-item.active").count()) === 1,
);

// --- view mode, the mark, and the jump history ---
// A third page, with two files and real links between them, because all three
// features are about *moving between reading positions* and one file cannot
// exercise any of them.
//
// View mode is asserted on measured width for a reason: it shipped hiding the
// sidebar with `display: none` while `#workspace` still declared two columns,
// which dropped `#main-wrap` into the 0px track and made the whole feature
// paint an empty window. Every check here was passing at the time — none of
// them looked at how wide the document was.
const nav = await newPage();
nav.on("pageerror", (e) => results.push("FAIL pageerror (nav): " + e.message));
nav.on("console", (m) => {
  if (m.type() === "error") results.push("FAIL console.error (nav): " + m.text());
});
await nav.addInitScript(({ base }) => {
  const tree = {
    name: "repo", is_dir: true, path: "/repo", rel: "",
    children: [
      { name: "doc.md", is_dir: false, path: "/repo/doc.md", rel: "doc.md", children: [] },
      { name: "other.md", is_dir: false, path: "/repo/other.md", rel: "other.md", children: [] },
    ],
  };
  // The `<h1>` names the file, which is how the assertions below tell the two
  // documents apart; the filler is what makes the offsets meaningful.
  const body = (p) =>
    `<h1 id="t">${p}</h1><a href="other.md#deep">to other deep</a><a href="#deep">to deep here</a>` +
    "<p>filler</p>".repeat(200) + `<h2 id="deep">deep</h2>` + "<p>filler</p>".repeat(200);
  window.__TAURI__ = {
    core: {
      async invoke(cmd, args) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return {
            palette: "Ctrl+F", palette_prev: "Ctrl+P", palette_next: "Ctrl+N",
            highlight: "Ctrl+H", send_stack: "Ctrl+Enter", toggle_stack: "Ctrl+O",
            toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B", toggle_view: "Ctrl+M",
            jump_top: "Home", jump_bottom: "End", set_mark: "m", jump_mark: "'",
            jump_back: "Ctrl+[", jump_forward: "Ctrl+]",
            find: "/", find_next: "n", find_prev: "Shift+N",
            next_file: "]", prev_file: "[",
            copy_stack: "Ctrl+C", settings: "Ctrl+,", save_annotation: "Ctrl+Y",
            quick_highlight: true,
          };
          case "get_theme": return { css: base, mode: "system", scheme: "dark", syntax_theme: null };
          case "initial_file": return "/repo/doc.md";
          case "render_markdown": return body((args && args.path) || "?");
          case "list_markdown_files": return tree;
          case "get_highlights": case "reanchor": case "get_stack": return [];
          // An opaque `h` + 16 hex id, the shape the backend mints. The
          // default null would let a numeric-id assumption pass unnoticed.
          case "add_highlight": return "h0123456789abcdef";
          default: return null;
        }
      },
    },
    event: { async listen() { return () => {}; } },
  };
}, { base });
await nav.goto(pathToFileURL(join(UI, "index.html")).href);
await nav.waitForFunction(() => document.getElementById("content").textContent.includes("/repo/doc.md"));

const where = () => nav.evaluate(() => ({
  file: document.querySelector("#content h1").textContent,
  top: Math.round(document.getElementById("content-scroll").scrollTop),
}));
const lastToast = () => nav.evaluate(() => document.getElementById("toast").textContent);
// Scrolls are `instant` because `scroll-behavior` would otherwise still be
// animating when the next line reads the offset.
const scrollTo = (top) =>
  nav.evaluate((t) => document.getElementById("content-scroll").scrollTo({ top: t, behavior: "instant" }), top);
const beat = (ms = 300) => nav.waitForTimeout(ms);

await nav.keyboard.press("Control+m");
await beat(150);
const vm = await nav.evaluate(() => ({
  content: Math.round(document.getElementById("content").getBoundingClientRect().width),
  pane: Math.round(document.getElementById("content-scroll").getBoundingClientRect().width),
  titlebar: getComputedStyle(document.getElementById("titlebar")).display,
}));
check("view mode leaves the document at full width", vm.pane === 1280 && vm.content > 400, JSON.stringify(vm));
check("view mode hides the titlebar", vm.titlebar === "none", vm.titlebar);

// Highlighting has to survive the chrome going away — it is the product loop,
// and view mode is where a reader spends the most time.
await nav.evaluate(() => {
  const r = document.createRange();
  r.selectNodeContents(document.querySelector("#content p"));
  const s = getSelection(); s.removeAllRanges(); s.addRange(r);
});
await nav.keyboard.press("h");
await beat(150);
check("the highlight flow still opens in view mode",
  await nav.locator("#annot-overlay.open").isVisible());
await nav.keyboard.press("Escape");
await beat(150);
check("Esc closes the annotation and view mode survives",
  await nav.evaluate(() => document.body.classList.contains("view-mode")));
await nav.keyboard.press("Escape");
await beat(150);
check("a second Esc leaves view mode",
  !(await nav.evaluate(() => document.body.classList.contains("view-mode"))));

await scrollTo(3000);
await beat(120);
await nav.keyboard.press("m");
await beat(120);
check("the set-mark key confirms immediately", (await lastToast()) === "Mark set", await lastToast());
await nav.keyboard.press("]");
await beat();
check("] opens the next file", (await where()).file === "/repo/other.md", JSON.stringify(await where()));
await nav.keyboard.press("'");
await beat(400);
check("' returns to the mark, file and offset",
  JSON.stringify(await where()) === JSON.stringify({ file: "/repo/doc.md", top: 3000 }),
  JSON.stringify(await where()));

await nav.keyboard.press("Control+[");
await beat(400);
check("jump back undoes the mark jump", (await where()).file === "/repo/other.md", JSON.stringify(await where()));
await nav.keyboard.press("Control+]");
await beat(400);
check("jump forward redoes it with the offset intact",
  JSON.stringify(await where()) === JSON.stringify({ file: "/repo/doc.md", top: 3000 }),
  JSON.stringify(await where()));

// The regression this guards: a cross-file section link is `openFile` followed
// by `scrollToFragment`, and pushing from inside both would take two presses to
// undo one jump — the intermediate frame being a document at offset 0 that
// nobody ever read.
await scrollTo(1200);
await beat(120);
await nav.evaluate(() =>
  [...document.querySelectorAll("#content a")].find((a) => a.textContent === "to other deep").click());
await beat(400);
const xf = await where();
check("a cross-file section link lands deep in the other file",
  xf.file === "/repo/other.md" && xf.top > 500, JSON.stringify(xf));
await nav.keyboard.press("Control+[");
await beat(400);
check("and one jump back undoes the whole of it",
  JSON.stringify(await where()) === JSON.stringify({ file: "/repo/doc.md", top: 1200 }),
  JSON.stringify(await where()));

await nav.keyboard.press("]");
await beat();
await nav.keyboard.press("Control+]");
await beat(150);
check("a new jump clears the forward trail", (await lastToast()) === "No later position", await lastToast());
for (let i = 0; i < 12; i++) { await nav.keyboard.press("Control+["); await beat(120); }
check("an exhausted trail says so rather than going silent",
  (await lastToast()) === "No earlier position", await lastToast());

// --- find in document ---
// `deep` appears three times in the fixture body — the two link labels and the
// `<h2>` — and smart case makes a lowercase pattern find all of them. Scrolled
// to the top first, because Enter picks the first match at or after where the
// reader is looking.
const findCount = () => nav.evaluate(() => document.getElementById("find-count").textContent);
const findMode = () => nav.evaluate(() => document.getElementById("find-mode").textContent);
const findCss = () => nav.evaluate(() => document.getElementById("find-css").textContent.length);
// Counts *painted* ranges, not whether the registry entry exists. The entries
// are registered once and mutated thereafter, so `get(...)` is truthy forever
// after the first search — asking that question is what let a stale paint pass.
const litCount = () => nav.evaluate(() => {
  const h = CSS.highlights.get("dreamd-find");
  return h ? h.size : 0;
});
// The painted ranges have to be *snapshotted before* the search is closed, or
// there is nothing left to inspect: clearing empties the highlight either way,
// so a check made afterwards passes whether or not the pixels were invalidated.
// That is the hole the first version of this bug slipped through. Holding the
// `Range` objects on `window` and asking afterwards whether they were collapsed
// is what distinguishes "removed from the model" from "and repainted".
const snapPainted = () => nav.evaluate(() => {
  window.__painted = [...(CSS.highlights.get("dreamd-find") || [])];
  return window.__painted.length;
});
const strayRanges = () =>
  nav.evaluate(() => window.__painted.filter((r) => !r.collapsed).length);
const barOpen = () => nav.locator("#find-bar.open").isVisible();
await nav.keyboard.press("Home");
await beat(150);
// Merely *declaring* the paint rules costs +27% on the forced layout after a 2MB
// render, so a session that never searches must not carry them. See
// `installFindCss` in app.js.
check("the paint rules are absent until the feature is used", (await findCss()) === 0);
await nav.keyboard.press("/");
await beat(120);
check("/ opens the find bar", await barOpen());
check("and installs the paint rules with it", (await findCss()) > 0);
check("and focuses its input",
  await nav.evaluate(() => document.activeElement === document.getElementById("find-input")));

// Typing must not search. Painting per keystroke flickered the highlight through
// the prefixes of the word and yanked the pane mid-word; Enter is the trigger.
await nav.fill("#find-input", "deep");
await beat(250);
check("typing paints nothing and counts nothing",
  (await litCount()) === 0 && (await findCount()) === "", await findCount());

await nav.keyboard.press("Enter");
await beat(200);
check("Enter runs the search", (await findCount()) === "1/3", await findCount());
check("and leaves the bar open", await barOpen());
// The whole reason the Custom Highlight API was chosen over `<mark>` wrapping:
// painting must not put a single node into the tree highlight anchoring reads.
check("and paints the matches without touching the DOM",
  await nav.evaluate(() =>
    CSS.highlights.get("dreamd-find").size === 3 &&
    document.querySelectorAll("#content mark").length === 0));
// Focus has to leave the input, or `n` types an `n`.
check("and hands focus back to the document",
  await nav.evaluate(() => document.activeElement !== document.getElementById("find-input")));

await nav.keyboard.press("n");
await beat(150);
check("n steps with the bar still open", (await findCount()) === "2/3", await findCount());
await nav.keyboard.press("Shift+N");
await beat(150);
check("Shift+N steps back", (await findCount()) === "1/3", await findCount());

// Esc is the only exit, and it takes the search with it.
check("there are painted ranges to lose", (await snapPainted()) === 3);
await nav.keyboard.press("Escape");
await beat(150);
check("Esc closes the bar", !(await barOpen()));
check("and removes every trace of the search",
  (await litCount()) === 0 && (await findCount()) === "", await findCount());
// The regression these two guard: the matches left the model but stayed on
// screen, going one at a time as clicks and scrolling repainted each strip —
// and after the first fix, one fragment could still linger a line from its word.
// Withdrawing the rules is what repaints; the collapse is the belt.
check("by withdrawing the paint rules, not just the matches", (await findCss()) === 0,
  `${await findCss()} chars of css`);
check("and leaving no uncollapsed range behind either", (await strayRanges()) === 0,
  `${await strayRanges()} stray`);
check("and does not leak through to view mode",
  !(await nav.evaluate(() => document.body.classList.contains("view-mode"))));

// No regex toggle: the pattern is read literally, and as a regex only where the
// literal finds nothing. A lone `(` is not valid regex at all, so it is a
// literal and simply finds nothing — there is no error state left to flag.
await nav.keyboard.press("/");
await beat(120);
await nav.fill("#find-input", "(");
await nav.keyboard.press("Enter");
await beat(200);
check("an unclosed group is a literal, not an error",
  (await findCount()) === "0/0" && (await findMode()) === "", await findCount());
// The sharpest case for "literal first". The `<h1>` renders the file path, so a
// single `.` occurs literally in the document exactly once — while `.` read as a
// regex would match every character and hit the 2000 cap. Literal must win.
await nav.fill("#find-input", ".");
await nav.keyboard.press("Enter");
await beat(200);
check("a literal match beats the regex reading",
  (await findCount()) === "1/1" && (await findMode()) === "",
  `${await findCount()} ${await findMode()}`);
// Nothing matches `de.p` literally, so it falls through to being the regex the
// reader plainly meant — and says so.
await nav.fill("#find-input", "de.p");
await nav.keyboard.press("Enter");
await beat(200);
check("and falls back to regex when the literal finds nothing",
  (await findCount()) === "1/3" && (await findMode()) === "regex",
  `${await findCount()} ${await findMode()}`);

// The ✕ button is the same exit as Esc.
await nav.locator("#find-close").click();
await beat(150);
check("the close button clears the search too",
  !(await barOpen()) && (await litCount()) === 0 && (await findCount()) === "");
check("and withdraws the paint rules the same way Esc does", (await findCss()) === 0,
  `${await findCss()} chars of css`);
// Re-opening has to put them back, or the next search paints nothing at all.
await nav.keyboard.press("/");
await beat(120);
check("re-opening reinstalls them", (await findCss()) > 0);
await nav.fill("#find-input", "deep");
await nav.keyboard.press("Enter");
await beat(200);
check("and a second search still paints", (await litCount()) === 3, `${await litCount()}`);
await nav.keyboard.press("Escape");
await beat(150);

// --- marks-changed: the agent push path ---
// A fourth page, and the first with a working event bus: every other stub
// returns a no-op unlisten, which is all they need. This one has to *deliver*,
// because the whole point of the path is that nothing in the window triggers it.
//
// The store here is mutable and the stub reads it per call, so `get_highlights`
// answers differently after an "agent" adds a mark — an emitter that always
// replayed the same list could not tell a repaint from a no-op.
const agent = await newPage();
agent.on("pageerror", (e) => results.push("FAIL pageerror (marks): " + e.message));
agent.on("console", (m) => {
  if (m.type() === "error") results.push("FAIL console.error (marks): " + m.text());
});
await agent.addInitScript(({ base }) => {
  const tree = {
    name: "repo", is_dir: true, path: "/repo", rel: "",
    children: [{ name: "doc.md", is_dir: false, path: "/repo/doc.md", rel: "doc.md", children: [] }],
  };
  // Distinct paragraphs so each quote lands in its own text node, and enough of
  // them that the run below crosses SCAN_THRESHOLD in both directions.
  const body = "<h1>/repo/doc.md</h1>" +
    Array.from({ length: 12 }, (_, i) => `<p>alpha bravo charlie ${i} delta echo</p>`).join("");
  // The full `Highlight` shape, not just what `applyHighlights` reads — the
  // stack card renders `file_path` and the line span too.
  const mark = (n) => ({
    id: "h00000000000000" + String(n).padStart(2, "0"),
    file_path: "/repo/doc.md",
    quote: `bravo charlie ${n - 1}`,
    prefix: "alpha ", suffix: " delta echo",
    line_start: n, line_end: n,
    state: "active",
  });
  const marks = [mark(1), mark(2)];
  // Non-empty *before boot*, which is the only way the `init()` check below can
  // fail: `#stack-badge` ships reading "0" in the markup, so a store that
  // started empty would paint the right answer whether or not anything asked.
  const stack = [{ highlight: mark(1), annotation: "seeded by an agent" }];
  // The agent's mutations, as the test drives them: add a mark, put a pair on
  // the stack. Deliberately *not* wired to the event — emitting is the test's
  // job, so a repaint that never happened shows up as a stale count.
  window.__MARK__ = (n) => marks.push(mark(n));
  window.__PAIR__ = (n, annotation) =>
    stack.push({ highlight: mark(n), annotation });

  const listeners = new Map();
  window.__EMIT__ = (name, payload) => {
    for (const fn of listeners.get(name) || []) fn({ payload });
  };

  window.__TAURI__ = {
    core: {
      async invoke(cmd) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return {
            palette: "Ctrl+F", palette_prev: "Ctrl+P", palette_next: "Ctrl+N",
            highlight: "Ctrl+H", send_stack: "Ctrl+Enter", toggle_stack: "Ctrl+O",
            toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B", toggle_view: "Ctrl+M",
            jump_top: "Home", jump_bottom: "End", set_mark: "m", jump_mark: "'",
            jump_back: "Ctrl+[", jump_forward: "Ctrl+]",
            find: "/", find_next: "n", find_prev: "Shift+N",
            next_file: "]", prev_file: "[",
            copy_stack: "Ctrl+C", settings: "Ctrl+,", save_annotation: "Ctrl+Y",
            quick_highlight: true,
          };
          case "get_theme": return { css: base, mode: "system", scheme: "dark", syntax_theme: null };
          case "initial_file": return "/repo/doc.md";
          case "render_markdown": return body;
          case "list_markdown_files": return tree;
          // Read live, not captured: this is the agent's mutation arriving.
          case "get_highlights": case "reanchor": return marks.map((m) => ({ ...m }));
          case "get_stack": return stack.map((p) => ({ ...p }));
          default: return null;
        }
      },
    },
    event: {
      async listen(name, fn) {
        if (!listeners.has(name)) listeners.set(name, new Set());
        listeners.get(name).add(fn);
        return () => listeners.get(name).delete(fn);
      },
    },
  };
}, { base });
await agent.goto(pathToFileURL(join(UI, "index.html")).href);
await agent.waitForFunction(() => document.getElementById("content").textContent.includes("/repo/doc.md"));

const hlCount = () => agent.evaluate(() => document.querySelectorAll("mark.hl").length);
// Emit and wait past the 80ms coalescing window plus the IPC turn behind it.
const emitMarks = async (payload) => {
  await agent.evaluate((p) => window.__EMIT__("marks-changed", p), payload);
  await agent.waitForTimeout(300);
};

const badge = () => agent.locator("#stack-badge").textContent();

// `init()` never called `refreshStack` — invisible while the store could only
// start empty, and a visible bug the moment an agent populates it before the
// window opens. The fixture's stack is seeded, so a boot that does not ask
// leaves the badge on the "0" the markup ships with.
check("boot paints the two seeded marks", (await hlCount()) === 2, `got ${await hlCount()}`);
check("init() refreshes the stack without being asked", (await badge()) === "1", await badge());

// THE double-wrap regression. `applyHighlights` never removed what was already
// in the DOM — it assumed the virgin tree `renderCurrent` hands it — so an
// in-place repaint wrapped every mark a second time, then a third. Three
// consecutive events, because two only catches the first doubling: the
// `normalize()` inside `clearHighlights` is what stops the *count* recovering
// while the marks quietly stop being findable, and only a third pass separates
// those. Remove `clearHighlights()` from `repaintHighlights` and this goes red.
const marksSeries = [];
for (let i = 0; i < 3; i++) {
  await emitMarks({ file_path: "/repo/doc.md", stack: false });
  marksSeries.push(await hlCount());
}
check(
  "mark count is stable across three consecutive marks-changed events",
  marksSeries.every((n) => n === 2),
  `2 seeded, then ${marksSeries.join(", ")}`,
);
// The marks must still be the *same* two elements' worth of text, not two
// survivors of a normalize that lost the rest.
check(
  "and the marks still cover their quotes",
  await agent.evaluate(() =>
    [...document.querySelectorAll("mark.hl")].map((m) => m.textContent).join("|") ===
    "bravo charlie 0|bravo charlie 1"),
  await agent.evaluate(() => [...document.querySelectorAll("mark.hl")].map((m) => m.textContent).join("|")),
);

// A repo-wide event (`file_path: null`) still concerns the open document.
await agent.evaluate(() => window.__MARK__(3));
await emitMarks({ file_path: null, stack: false });
check("a null file_path repaints the open document", (await hlCount()) === 3, `got ${await hlCount()}`);

// A named file that is not the open one must not repaint it. Asserted against a
// store that has *changed*, so a repaint would be visible if it happened.
await agent.evaluate(() => window.__MARK__(4));
await emitMarks({ file_path: "/repo/other.md", stack: false });
check("a marks-changed for another file leaves this one alone", (await hlCount()) === 3, `got ${await hlCount()}`);

// `stack: true` is the other half of the payload.
await agent.evaluate(() => window.__PAIR__(2, "and this?"));
await emitMarks({ file_path: null, stack: true });
check("stack: true refreshes the stack", (await badge()) === "2", await badge());
check("and the same event repainted the marks too", (await hlCount()) === 4, `got ${await hlCount()}`);

// Coalescing: a burst inside the 80ms window is one repaint, not six. Asserted
// on the count staying correct rather than on a call tally — six un-coalesced
// repaints would each be correct in isolation, so this is a smoke check that
// the burst does not leave the overlay wrong, not proof of the debounce.
await agent.evaluate(() => {
  for (let i = 0; i < 6; i++) window.__EMIT__("marks-changed", { file_path: null, stack: true });
});
await agent.waitForTimeout(400);
check("a burst of events settles to one correct overlay", (await hlCount()) === 4, `got ${await hlCount()}`);

await browser.close();
console.log(results.join("\n"));
const failed = results.filter((r) => r.startsWith("FAIL")).length;
console.log(`\n${results.length - failed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
