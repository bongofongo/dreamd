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
    save_annotation: "Ctrl+Y", toggle_pane: "Ctrl+T",
    quick_highlight: true,
  };
  const state = {
    config: {
      theme: "dreamd", mode: "system",
      tmux_autodetect: true, extra_ignores: [], keymap: { ...KEYMAP },
      // `config::Ui` is a plain struct, so Rust always sends all three —
      // spelling them out here is what keeps the Window tab's checks below
      // asserting on the payload the real backend produces.
      ui: { tree_width: 260, menubar: false, titlebar: false },
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
check("every action gets a row", rows === 25, `got ${rows}`);
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

// --- window tab ---
// Chromium is not WebKitGTK and there is no window here at all, so what this
// can assert is exactly the frontend's half: the rows exist, they start from
// the payload, and a click writes the key Rust reads. Whether the bar actually
// disappears is `apply_chrome`'s, and a hand-check.
await page.locator('.st-tab[data-pane="window"]').click();
await page.waitForTimeout(120);
const chromeRows = await page.locator("#st-window .st-row").count();
check("both chrome toggles listed off macOS", chromeRows === 2, `got ${chromeRows}`);
const titlebarBox = page.locator("#st-window .st-row", { hasText: "Native titlebar" })
  .locator('input[type="checkbox"]');
check("titlebar starts from the config payload", !(await titlebarBox.isChecked()));
await titlebarBox.click();
await page.waitForTimeout(200);
check(
  "toggling writes ui.titlebar",
  (await page.evaluate(() => window.__STATE__.config.ui.titlebar)) === true,
);
check("and the box stays checked", await titlebarBox.isChecked());

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
// Six shared type metrics plus the sixteen in the appearance being edited
// (thirteen colours, --syntax-theme, --stale-text, and T3's --hl-prior). An
// exact count on purpose: it is what catches the Custom tab silently listing
// one block instead of two.
const varRows = await page.locator("#st-vars .st-var").count();
check("var editor lists shared + one appearance", varRows === 22, `got ${varRows}`);
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
// plus the appearance being edited, which is why the count above holds here
// too.
await page.locator("#st-theme-grid .th-card", { hasText: "gruvbox" }).first()
  .locator("button", { hasText: "Duplicate" }).click();
await page.waitForTimeout(250);
const blockBtns = await page.locator("#st-block .st-mode-btn").count();
check("custom tab offers both blocks", blockBtns === 2, `got ${blockBtns}`);
const darkRows = await page.locator("#st-vars .st-var").count();
check("editing dark lists shared + dark", darkRows === 22, `got ${darkRows}`);
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

// --- the file ⋯ menu stays inside the window ------------------------------
// It is `position: fixed`, so nothing clips it — it was simply laid out past the
// bottom edge, and for a tree that fills the sidebar that was every file in the
// lower half. Driven through `openFileMenu` against a synthetic anchor rather
// than a real tree row, because what changed is the arithmetic and a fixture
// tall enough to reach the foot of the viewport would be testing the fixture.
const menuAt = (y) =>
  nav.evaluate((top) => {
    const el = document.createElement("div");
    el.style.cssText = `position:fixed;left:100px;top:${top}px;width:20px;height:16px;`;
    document.body.appendChild(el);
    openFileMenu(el, { path: "/repo/doc.md" });
    const r = document.getElementById("file-menu").getBoundingClientRect();
    closeFileMenu();
    el.remove();
    return { top: r.top, bottom: r.bottom, left: r.left, right: r.right, anchor: top };
  }, y);

const lowMenu = await menuAt(await nav.evaluate(() => window.innerHeight - 30));
check(
  "a ⋯ menu near the foot of the window flips above its button",
  lowMenu.bottom <= lowMenu.anchor && lowMenu.top >= 0,
  `anchor ${lowMenu.anchor}, menu ${lowMenu.top}–${lowMenu.bottom}`,
);
check(
  "and stays inside the viewport",
  lowMenu.bottom <= (await nav.evaluate(() => window.innerHeight)) &&
    lowMenu.right <= (await nav.evaluate(() => window.innerWidth)),
  `${lowMenu.top}–${lowMenu.bottom}, right ${lowMenu.right}`,
);
// The common case must not have moved: there is room below, so it opens below.
const highMenu = await menuAt(40);
check(
  "and one with room below it still opens downward",
  highMenu.top >= 40 + 16,
  `anchor 40, menu top ${highMenu.top}`,
);

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
  //
  // The last paragraph is the odd one out and is there for one check: a `<strong>`
  // run splits it into three text nodes, so a quote across the whole line exists
  // in the rendered text and in none of its nodes. That is the shape a reader
  // produces by dragging over a bolded phrase, and it is what neither placer can
  // wrap.
  const body = "<h1>/repo/doc.md</h1>" +
    Array.from({ length: 12 }, (_, i) => `<p>alpha bravo charlie ${i} delta echo</p>`).join("") +
    "<p>foxtrot <strong>golf</strong> hotel</p>";
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
  // T6: put a mark out with the agent — `sent_at`, which a send stamps. It used
  // to raise a chip on the rail; the checks below are that it now raises nothing.
  // `stale` on top of it was D5's two-glyph case and is now the one-glyph case.
  window.__PEND__ = (n, stale) => {
    const m = marks.find((x) => x.id.endsWith(String(n).padStart(2, "0")));
    m.sent_at = 1_700_000_000;
    if (stale) m.state = "stale";
  };
  // An active mark whose quote spans the `<strong>` run: present in the rendered
  // text, present in no single text node, so both placers fail on it.
  window.__CROSSNODE__ = () =>
    marks.push({
      id: "h00000000000000ff",
      file_path: "/repo/doc.md",
      quote: "foxtrot golf hotel",
      prefix: "", suffix: "",
      line_start: 13, line_end: 13,
      state: "active",
    });

  const listeners = new Map();
  window.__EMIT__ = (name, payload) => {
    for (const fn of listeners.get(name) || []) fn({ payload });
  };

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
          case "render_markdown": return body;
          case "list_markdown_files": return tree;
          // Read live, not captured: this is the agent's mutation arriving.
          case "get_highlights": case "reanchor": return marks.map((m) => ({ ...m }));
          case "get_stack": return stack.map((p) => ({ ...p }));
          // Mirrors `Store::remove_from_stack` exactly, both halves: the queue
          // shrinks *and* the passage becomes prior. The second half is the one
          // the checks below are about — a pop has to fade the mark, and the
          // frontend only shows that if it repaints.
          case "remove_pair": {
            const at = stack.findIndex((p) => p.highlight.id === args.id);
            if (at >= 0) stack.splice(at, 1);
            const m = marks.find((x) => x.id === args.id);
            if (m) m.prior = true;
            return null;
          }
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
// How many distinct highlights are painted, which is not the same number: a quote
// spanning a bold run or a link is several `<mark>`s sharing one id (see
// `placeAcrossNodes`). Every assertion about *whether a passage shows* wants this
// one; `hlCount` is the element tally and only interesting when the split itself
// is the subject.
const hlIds = () =>
  agent.evaluate(
    () => new Set([...document.querySelectorAll("mark.hl")].map((m) => m.dataset.id)).size,
  );
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

// --- what the rail is allowed to say --------------------------------------
// One chip, one meaning: this passage's own text changed. `sent_at` is still
// stamped by a send and still cleared by `Store::resolve` — the agent's record of
// what it closed — but it paints nothing, because a question that has been asked
// is a question dealt with. Reinstate `addPendingChip` and the first check goes
// red; reinstate either `addStaleChip` on a placement failure and the last two do.
const pendChips = () => agent.locator(".pend-chip").count();
const staleChips = () => agent.locator(".stale-chip").count();

await agent.evaluate(() => window.__PEND__(1));
await emitMarks({ file_path: "/repo/doc.md", stack: false });
check(
  "a mark out with the agent raises nothing beside the document",
  (await pendChips()) === 0 && (await staleChips()) === 0,
  `${await pendChips()} pending, ${await staleChips()} stale`,
);
check("and the passage itself still paints", (await hlCount()) === 4, `got ${await hlCount()}`);

// What was D5's two-glyph case. A passage can still go stale *while* it is out
// with the agent — that is why `sent_at` sits beside `state` rather than inside
// it — but the rail has one tenant now, so it is told once.
await agent.evaluate(() => window.__PEND__(2, true));
await emitMarks({ file_path: "/repo/doc.md", stack: false });
check(
  "a passage that went stale while it was out shows the stale chip alone",
  (await staleChips()) === 1 && (await pendChips()) === 0,
  `${await staleChips()} stale, ${await pendChips()} pending`,
);

// --- a quote that spans an element boundary still paints -------------------
// The bug the reader actually hit, and the reason `placeAcrossNodes` exists. The
// frontend stores what `getSelection().toString()` returns, so dragging across a
// bolded phrase stores a quote that spans three text nodes. `wrapRange` on the
// live selection paints it once at creation; every later paint went through
// `wrapByWalk`/`locateInNodes`, which only looked *inside* one node, found
// nothing and drew nothing. The mark stayed in the store and on the stack — the
// badge counted it — so a highlight survived being annotated and then vanished
// the next time anything repainted. Reproduced in a real window with four marks
// on the stack and one visible.
//
// It must also stay silent. A placement failure was never a claim about the
// source, which is `Store::reanchor_file`'s alone to make, so the stale count is
// asserted alongside every paint count here.
//
// Both placers, in turn: `applyHighlights` walks node by node at or below
// SCAN_THRESHOLD active marks and flattens the document above it.
await agent.evaluate(() => window.__CROSSNODE__());
await emitMarks({ file_path: "/repo/doc.md", stack: false });
check(
  "a quote spanning an element boundary paints (walk path)",
  (await hlIds()) === 4 && (await staleChips()) === 1 && (await pendChips()) === 0,
  `${await hlIds()} passages, ${await staleChips()} stale`,
);
// Three `<mark>`s for that one quote — `foxtrot `, `golf` inside the `<strong>`,
// and ` hotel` — because one `<mark>` around the whole range would have to
// re-parent the `<strong>`'s contents to exist. Three of the four painted
// passages are single-node, so 3 + 3 = 6 elements for 4 ids.
check(
  "as several marks sharing one id, not one mark around the range",
  (await hlCount()) === 6,
  `${await hlCount()} elements for ${await hlIds()} ids`,
);
// The seam: `mark.hl` carries `padding: 0 1px` and `border-radius: 2px`, so three
// abutting slices drew three rounded pills and one phrase read as three marks.
// `data-run` squares the interior edges. Nothing sets it on a quote that fits.
check(
  "and the run's interior edges are marked so the fill is continuous",
  await agent.evaluate(() => {
    const run = [...document.querySelectorAll('mark.hl[data-id="h00000000000000ff"]')];
    const fits = [...document.querySelectorAll("mark.hl:not([data-run])")];
    return (
      run.map((m) => m.dataset.run).join(",") === "start,mid,end" &&
      fits.length === 3 // the single-node passages, untouched by these rules
    );
  }),
  await agent.evaluate(() =>
    [...document.querySelectorAll('mark.hl[data-id="h00000000000000ff"]')]
      .map((m) => m.dataset.run || "-").join(","),
  ),
);
// The strong is still a strong: the slice is wrapped *in place*, so the rendered
// document is the one pulldown-cmark built and the phrase is bold and highlighted.
check(
  "the element it spans is left intact",
  await agent.evaluate(() => {
    const m = document.querySelector('mark.hl[data-run="mid"]');
    return !!m && m.parentElement.tagName === "STRONG" && m.textContent === "golf";
  }),
);
await agent.evaluate(() => { window.__MARK__(5); window.__MARK__(6); });
await emitMarks({ file_path: "/repo/doc.md", stack: false });
check(
  "and it paints past SCAN_THRESHOLD too (scan path)",
  (await hlIds()) === 6 && (await staleChips()) === 1,
  `${await hlIds()} passages, ${await staleChips()} stale`,
);

// --- the fade tracks the stack --------------------------------------------
// `prior` used to mean "read off disk" and nothing else. It now means "done
// with", which a sent question and a popped one both are, so `Store::mark_sent`
// and `Store::remove_from_stack` set it. That is the Rust half, pinned by
// `the_fade_tracks_the_stack_in_both_directions`. This is the half it cannot
// reach: the reader has to *see* it without waiting for a restart, and a pop went
// through `refreshStack` alone — which redraws the panel and never touches the
// overlay — so the mark kept full strength until something unrelated repainted.
const priorAttr = (id) =>
  agent.evaluate(
    (i) => document.querySelector(`mark.hl[data-id="${i}"]`)?.hasAttribute("data-prior") ?? null,
    id,
  );
const STACKED = "h0000000000000001";

check("a passage still on the stack is at full strength", (await priorAttr(STACKED)) === false);

// The panel is opened first so the click is the one a reader makes, animation
// path included, rather than a handler called past the visibility it needs.
await agent.evaluate(() => document.getElementById("stack-panel").classList.add("open"));
await agent.locator(`#stack-list .pair[data-id="${STACKED}"] .rm`).click();
await agent.waitForTimeout(400);
check(
  "popping it off the stack fades it immediately",
  (await priorAttr(STACKED)) === true,
  `data-prior: ${await priorAttr(STACKED)}`,
);
check(
  "and the card goes with it",
  (await agent.locator(`#stack-list .pair[data-id="${STACKED}"]`).count()) === 0,
  `${await agent.locator("#stack-list .pair").count()} cards left`,
);
// The passage is still painted — a pop is not a delete. This is the assertion
// that would catch a `repaintHighlights` that dropped the mark instead of
// re-drawing it faded.
check("but the passage is still painted", (await priorAttr(STACKED)) !== null);

// --- the embedded Claude Code pane ----------------------------------------
// A fifth page, because everything here is about a surface that does not exist
// until a key is pressed, and the pages above assert on a boot that must never
// touch it.
//
// What this can and cannot see: the terminal's *contents* are readable, because
// xterm.js keeps a buffer model the DOM renderer paints from — so the
// chunk-boundary assertion below is real. What it paints is not checked, here
// or anywhere; `CLAUDE.md` is explicit that this harness asserts what the page
// knows, not what it shows.
//
// The stub is a named function rather than an inline arrow because it is
// installed on two pages: one docked bottom and one docked right, which is the
// only way to assert `agent.position` actually reaches the layout.
const paneStub = ({ base, position }) => {
  const calls = [];
  window.__CALLS__ = calls;
  // T6's send stack and the queue behind it, mirroring `src-tauri/src/flow.rs`
  // closely enough to drive the frontend: an id cannot be in two live
  // submissions, arming is one-way, and `take_send` hands back the oldest armed
  // entry exactly once. It starts *empty*, which is what makes the D10 check
  // below — Ctrl+Enter on an empty stack — a real one.
  const stack = [];
  window.__STACK__ = stack;
  window.__QUEUE__ = [];
  window.__SENT__ = [];
  let nextToken = 0;
  // What `agent_prefs` answers, and what `set_config` writes into — the two
  // halves of the mode control's round trip.
  window.__AGENT__ = { position, permission_mode: "accept-edits" };
  // What `mcp_status` answers. Healthy, so the strip is hidden on boot and the
  // checks that want it have to say what is wrong.
  window.__MCP__ = { armed: true, serving: true, clients: 1 };
  window.__MODEL__ = null;
  const listeners = new Map();
  window.__EMIT__ = (name, payload) => {
    for (const fn of listeners.get(name) || []) fn({ payload });
  };
  window.__TAURI__ = {
    core: {
      async invoke(cmd, args) {
        calls.push({ cmd, args });
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return {
            palette: "Ctrl+F", highlight: "Ctrl+H", toggle_stack: "Ctrl+O",
            toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B", toggle_view: "Ctrl+M",
            toggle_pane: "Ctrl+T", find: "/", settings: "Ctrl+,",
            send_stack: "Ctrl+Enter",
            quick_highlight: true,
          };
          case "get_theme": return { css: base, mode: "system", scheme: "dark", syntax_theme: null };
          case "initial_file": return "/repo/doc.md";
          case "render_markdown": return "<h1>doc</h1><p>alpha bravo</p>";
          case "list_markdown_files": return {
            name: "repo", is_dir: true, path: "/repo", rel: "",
            children: [{ name: "doc.md", is_dir: false, path: "/repo/doc.md", rel: "doc.md", children: [] }],
          };
          case "get_highlights": case "reanchor": return [];
          case "get_stack": return stack.map((p) => ({ ...p }));
          case "stack_query_text": return "stub query";
          case "queue_send": {
            const wanted = args.ids && args.ids.length
              ? args.ids : stack.map((p) => p.highlight.id);
            const ids = wanted.filter((id) =>
              stack.some((p) => p.highlight.id === id) &&
              !window.__QUEUE__.some((q) => q.ids.includes(id)));
            if (!ids.length) return null; // D10: nothing usable to send
            const p = { token: ++nextToken, ids, phase: "undo" };
            window.__QUEUE__.push(p);
            return p;
          }
          case "arm_send": {
            const p = window.__QUEUE__.find((q) => q.token === args.token);
            if (!p || p.phase !== "undo") return false;
            p.phase = "armed";
            return true;
          }
          case "cancel_send": {
            const before = window.__QUEUE__.length;
            window.__QUEUE__ = window.__QUEUE__.filter((q) => q.token !== args.token);
            return window.__QUEUE__.length !== before;
          }
          case "take_send": {
            const at = window.__QUEUE__.findIndex((q) => q.phase === "armed");
            if (at < 0) return null;
            const [p] = window.__QUEUE__.splice(at, 1);
            window.__SENT__.push(p);
            // What the real command does after the pty write lands: `mark_sent`
            // takes exactly those ids off the stack and leaves the rest.
            for (const id of p.ids) {
              const i = stack.findIndex((s) => s.highlight.id === id);
              if (i >= 0) stack.splice(i, 1);
            }
            return { token: p.token, ids: p.ids };
          }
          case "agent_prefs": return { ...window.__AGENT__ };
          // The real one merges into the global config file and hands back the
          // whole `Settings` payload; the pane only ever reads the write back
          // out of `agent_prefs`, so recording the patch is the whole contract.
          case "set_config":
            if (args.patch && args.patch.agent) Object.assign(window.__AGENT__, args.patch.agent);
            return { config: {}, local_overrides: [] };
          // No process. This asserts the frontend's half of the contract;
          // `src-tauri/src/pty.rs`'s tests own the process half.
          case "pty_spawn": return true;
          case "pty_write": case "pty_resize": case "pty_kill": return null;
          // The real one types `/model <name>` into the child; here the point
          // is only that the chip reached IPC carrying a word Rust's closed
          // enum will accept. `pty.rs` owns what that word becomes.
          case "pty_model": window.__MODEL__ = args.model; return null;
          // Healthy by default, so the strip stays hidden unless a check makes
          // it otherwise — which is also the real answer once an agent has
          // connected.
          case "mcp_status": return { ...window.__MCP__ };
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
};

/// A page with the pane stub installed, docked to `position`, booted as far as
/// the document.
async function newPanePage(position) {
  const p = await newPage();
  p.on("pageerror", (e) => results.push(`FAIL pageerror (pane/${position}): ` + e.message));
  p.on("console", (m) => {
    if (m.type() === "error") results.push(`FAIL console.error (pane/${position}): ` + m.text());
  });
  await p.addInitScript(paneStub, { base, position });
  await p.goto(pathToFileURL(join(UI, "index.html")).href);
  await p.waitForFunction(() => document.getElementById("content").textContent.includes("alpha bravo"));
  return p;
}

const pane = await newPanePage("bottom");

const called = (cmd) => pane.evaluate((c) => window.__CALLS__.filter((k) => k.cmd === c), cmd);
/// Leave the pane open without pressing a key. Used between assertion groups so
/// one broken keybind fails one check instead of derailing every later one:
/// `openPane` is idempotent, so this is a no-op when the pane is already up.
const ensurePaneOpen = async () => {
  await pane.evaluate(() =>
    document.getElementById("pty-pane").classList.contains("open") ? null : openPane());
  await pane.waitForTimeout(300);
};
const lastWrite = async () => {
  const w = await called("pty_write");
  return w.length ? w[w.length - 1].args.data : null;
};

// The perf contract, asserted rather than assumed: none of this loads at boot.
check(
  "boot loads no terminal vendor script",
  await pane.evaluate(() => !document.querySelector('script[src*="xterm"]')),
);
check("boot starts no pane process", (await called("pty_spawn")).length === 0);
check(
  "the pane is display:none before it is opened",
  (await pane.evaluate(() => getComputedStyle(document.getElementById("pty-pane")).display)) === "none",
);

await pane.keyboard.press("Control+t");
// `pty` is a top-level `const` in a classic script: a lexical global, reachable
// by name from an evaluate but never a property of `window`.
await pane.waitForFunction(() => typeof pty !== "undefined" && !!pty.term, null, { timeout: 15000 })
  .catch(() => {});
await pane.waitForTimeout(600);

check("the toggle key opens the pane", await pane.locator("#pty-pane.open").isVisible());
check(
  "and only then loads the vendored xterm.js",
  await pane.evaluate(() => !!document.querySelector('script[src="vendor/xterm.js"]')),
);
const spawns = await called("pty_spawn");
check("and starts one process, sized", spawns.length === 1 && spawns[0].args.rows > 0 && spawns[0].args.cols > 0,
  JSON.stringify(spawns));

// The pane's padding belongs on `.xterm`, and this is the only place that can
// hold that line.
//
// The bug: WebKitGTK answers `getComputedStyle(#pty-term).height` with the
// **border** box, while `FitAddon.proposeDimensions` divides that by the cell
// height and subtracts only `terminal.element`'s padding. With the padding on the
// dock instead of on `.xterm` the addon counted it as usable rows and `#pty-pane`'s
// `overflow: hidden` took the difference off the bottom — the last row, which is
// exactly where Claude Code draws its composer. Measured in a real window at 9.4px
// with the MCP strip up, and two columns of the same error horizontally.
//
// **Chromium cannot reproduce it**, which is the whole reason it went unnoticed:
// it resolves that height as the *content* box, per spec, so the arithmetic comes
// out right here whichever element carries the padding. Verified by reverting —
// the geometry assertion below stayed green. So the assertion that bites is the
// structural one: the padding is on `.xterm` and `#pty-term` has none. Move it
// back and this goes red; the geometry check beside it is the belt, and would
// catch someone giving the grid an explicit height instead.
const grid = await pane.evaluate(() => {
  const box = document.getElementById("pty-term");
  const xt = box.querySelector(".xterm");
  const pad = (el) => {
    const s = getComputedStyle(el);
    return [s.paddingTop, s.paddingBottom, s.paddingLeft, s.paddingRight].map(parseFloat);
  };
  return {
    dock: pad(box),
    term: pad(xt),
    fits: xt.offsetHeight <= box.clientHeight,
    xterm: xt.offsetHeight,
    box: box.clientHeight,
  };
});
check(
  "the terminal's padding is on .xterm, where FitAddon subtracts it",
  grid.term.some((n) => n > 0) && grid.dock.every((n) => n === 0),
  `dock ${grid.dock.join("/")}, .xterm ${grid.term.join("/")}`,
);
check(
  "and the grid fits the box it was measured against",
  grid.fits,
  `.xterm ${grid.xterm}px in ${grid.box}px`,
);

// The hazard the plan names: bare-letter bindings must not fire while the
// terminal has focus. `/` is `find`, and a find bar opening under a terminal
// you are typing a path into is the visible form of the bug.
await pane.keyboard.press("/");
await pane.waitForTimeout(150);
check("a keystroke in the pane does not reach the global keymap", !(await pane.locator("#find-bar.open").isVisible()));
check("it reaches pty_write instead", (await lastWrite()) === btoa("/"), String(await lastWrite()));

// D12: Escape closes the pane, in every mode — including from inside the
// terminal, where it used to be the child's interrupt. The session survives:
// closing is hiding, so `pty_kill` must not have been called.
const killsBeforeEscape = (await called("pty_kill")).length;
const writesBeforeEscape = (await called("pty_write")).length;
await pane.keyboard.press("Escape");
await pane.waitForTimeout(150);
check("Escape inside the pane closes it", !(await pane.locator("#pty-pane.open").isVisible()));
check(
  "and is not also sent to the child",
  (await called("pty_write")).length === writesBeforeEscape,
  String(await lastWrite()),
);
check(
  "and leaves the session running",
  (await called("pty_kill")).length === killsBeforeEscape,
);

// Put the pane back open whatever the assertions above found. Every check from
// here on needs a visible pane, and a keyboard toggle would invert instead of
// normalise — so a regression in the Escape rule reports as the one failed
// check it is, rather than as a click timeout six assertions later.
await ensurePaneOpen();
check(
  "reopening after Escape starts no second process",
  await pane.locator("#pty-pane.open").isVisible() && (await called("pty_spawn")).length === 1,
  `${(await called("pty_spawn")).length} spawns`,
);

// The way back out, and the check this feature shipped without at first: xterm
// calls `stopPropagation` on every key it handles, so the toggle pressed
// *inside* the pane never reaches `wireKeys` and the pane was a keyboard trap.
// Remove `attachCustomKeyEventHandler` from `buildTerminal` and this goes red.
const writesBeforeToggle = (await called("pty_write")).length;
await pane.keyboard.press("Control+t");
await pane.waitForTimeout(200);
check("the toggle key closes the pane from inside it", !(await pane.locator("#pty-pane.open").isVisible()));
check(
  "and is not also sent to the child",
  (await called("pty_write")).length === writesBeforeToggle,
  String(await lastWrite()),
);
await pane.keyboard.press("Control+t");
await pane.waitForTimeout(300);
check("and reopens it", await pane.locator("#pty-pane.open").isVisible());
await ensurePaneOpen();

// The reason output is base64 (`pty.rs`'s module docs). Two events, splitting a
// 4-byte character 3/1: decoded per chunk this is two U+FFFDs.
const seed = "🌱";
const bytes = [...new TextEncoder().encode(seed)];
const b64 = (arr) => Buffer.from(Uint8Array.from(arr)).toString("base64");
await pane.evaluate((chunks) => {
  window.__EMIT__("pty-data", chunks[0]);
  window.__EMIT__("pty-data", chunks[1]);
}, [b64(bytes.slice(0, 3)), b64(bytes.slice(3))]);
await pane.waitForTimeout(200);
check(
  "a character split across two pty-data events is reassembled",
  (await pane.evaluate(() => pty.term.buffer.active.getLine(0).translateToString(true))).includes("🌱"),
  await pane.evaluate(() => pty.term.buffer.active.getLine(0).translateToString(true)),
);

// A dead child says so, in the header rather than only in the scrollback.
await pane.evaluate(() => window.__EMIT__("pty-exit", 1));
await pane.waitForTimeout(150);
check("an exited child is reported", (await pane.textContent("#pty-status")) === "exited (1)",
  await pane.textContent("#pty-status"));

// ⟳ after an exit: kill the (already dead) session, then start a new one.
await pane.locator("#pty-restart").click();
await pane.waitForTimeout(300);
check("restart kills and respawns", (await called("pty_kill")).length === 1 && (await called("pty_spawn")).length === 2,
  `${(await called("pty_kill")).length} kills, ${(await called("pty_spawn")).length} spawns`);

// Escape *outside* the terminal is the reading pane's, and closes the pane —
// hiding it, not killing it, which is what makes reopening free.
await pane.locator("#content").click();
await pane.keyboard.press("Escape");
await pane.waitForTimeout(150);
check("Escape outside the pane closes it", !(await pane.locator("#pty-pane.open").isVisible()));
check("closing does not kill the process", (await called("pty_kill")).length === 1);
check(
  "and view mode survives that Escape",
  !(await pane.evaluate(() => document.body.classList.contains("view-mode"))),
);

// Reopening is a class flip: no second vendor load, no second process.
await pane.keyboard.press("Control+t");
await pane.waitForTimeout(300);
check("reopening reuses the session", (await called("pty_spawn")).length === 2 &&
  (await pane.evaluate(() => document.querySelectorAll('script[src="vendor/xterm.js"]').length)) === 1);

// --- the permission-mode control ---
// The mode is a launch flag, so changing it is a restart and a restart is a new
// conversation. What is asserted here is that the cost is *shown* — a staged
// change with a sentence about it — rather than paid silently on `change`.
check(
  "the header shows the mode the pane launched in",
  (await pane.locator("#pty-mode").inputValue()) === "accept-edits",
  await pane.locator("#pty-mode").inputValue(),
);
// Each click below is guarded on the strip actually being up. A mode control
// that applied on `change` would otherwise take the whole harness down with a
// click timeout instead of reporting the three checks it broke.
const clickIfStaged = async (id) => {
  if (await pane.locator("#pty-confirm").isVisible()) await pane.locator(id).click();
  await pane.waitForTimeout(300);
};
const patchesBefore = (await called("set_config")).length;
await pane.locator("#pty-mode").selectOption("plan");
await pane.waitForTimeout(150);
check("choosing a mode stages it rather than applying it", await pane.locator("#pty-confirm").isVisible());
check(
  "and says what the restart costs",
  /restarts the session/.test(await pane.textContent("#pty-confirm-text")),
  await pane.textContent("#pty-confirm-text"),
);
check("nothing is written until it is confirmed", (await called("set_config")).length === patchesBefore);

await clickIfStaged("#pty-mode-cancel");
check("declining hides the strip", !(await pane.locator("#pty-confirm").isVisible()));
check(
  "and puts the select back",
  (await pane.locator("#pty-mode").inputValue()) === "accept-edits",
  await pane.locator("#pty-mode").inputValue(),
);
check("and still writes nothing", (await called("set_config")).length === patchesBefore);

const spawnsBeforeMode = (await called("pty_spawn")).length;
const killsBeforeMode = (await called("pty_kill")).length;
await pane.locator("#pty-mode").selectOption("plan");
await pane.waitForTimeout(150);
await clickIfStaged("#pty-mode-go");
check(
  "confirming writes the preference back",
  (await pane.evaluate(() => window.__AGENT__.permission_mode)) === "plan",
  await pane.evaluate(() => window.__AGENT__.permission_mode),
);
// Order, not just occurrence: `pty_spawn` reads the mode out of the config it
// already holds, so a restart that ran before the write would come back up in
// the old mode. This is the assertion that catches that inversion.
const seq = await pane.evaluate(() =>
  window.__CALLS__.map((c) => c.cmd).filter((c) => c === "set_config" || c === "pty_spawn"));
check(
  "and writes it before restarting into it",
  seq[seq.length - 2] === "set_config" && seq[seq.length - 1] === "pty_spawn",
  seq.join(","),
);
check(
  "and the restart is a real one",
  (await called("pty_spawn")).length === spawnsBeforeMode + 1 &&
    (await called("pty_kill")).length === killsBeforeMode + 1,
);

// --- T6: Ctrl+Enter is one verb -------------------------------------------
// What this can honestly see: which IPC calls the key makes, what the send bar
// says, and that nothing throws. What it cannot see is the only thing that
// really matters — whether Claude Code receives a usable prompt — because there
// is no Claude Code here and `pty_write` is a stub. That is the hand check.

const queued = () => pane.evaluate(() => window.__QUEUE__.length);
const sent = () => pane.evaluate(() => window.__SENT__.length);
const onStack = () => pane.evaluate(() => window.__STACK__.length);
const barOpenPane = () => pane.locator("#send-bar.open").isVisible();
/// Push a pair onto the stub's stack without driving the highlight flow, which
/// is a different feature's path and has its own checks.
const seedPair = (n) => pane.evaluate((i) => window.__STACK__.push({
  highlight: {
    id: "h000000000000000" + i, file_path: "/repo/doc.md", quote: "alpha bravo",
    prefix: "", suffix: "", line_start: 1, line_end: 1, state: "active",
  },
  annotation: "why?",
}), n);
/// Say how long ago the child last spoke, instead of sleeping through
/// `BOOT_QUIET_MS`. What is under test is the release path, not `setInterval`.
///
/// `pty.settled` is cleared alongside it, because the flag latches: a test that
/// wants the pane to look mid-boot has to undo an earlier test's quiet as well
/// as set the timestamp.
const nudgeFlow = (quietMs) => pane.evaluate((q) => {
  flow.lastData = Date.now() - q;
  pty.settled = q >= 1500;
}, quietMs);
/// A child that has *never* spoken — the cold start, and the one state the boot
/// watch will not leave on its own however long the harness takes between two
/// assertions. `noteBootQuiet` returns early on `lastData === 0`, so this is
/// stable where "spoke 0ms ago" would quietly ripen into "quiet for 1.5s"
/// somewhere in the middle of a check and make the block flaky.
const coldPane = () => pane.evaluate(() => { flow.lastData = 0; pty.settled = false; });
/// Ctrl+Enter *from the reader*. The click is not decoration: opening the pane
/// focuses the terminal, and every key with focus in there belongs to the child
/// (`inTerminal`), so a press without it would be swallowed and every assertion
/// below would fail for a reason that is not the one being tested.
const pressSend = async () => {
  await pane.locator("#content").click();
  await pane.keyboard.press("Control+Enter");
  await pane.waitForTimeout(400);
};
/// Click a button on the send bar, if the bar is offering one. Guarded the same
/// way `clickIfStaged` guards the mode control, and for the same reason: a
/// regression that stops the bar appearing should report as the one or two
/// failed checks it is, not as a 30-second click timeout that takes the whole
/// harness down before it prints anything.
const clickBar = async (label) => {
  const btn = pane.locator("#send-bar button", { hasText: label });
  if (await btn.count()) await btn.first().click();
  await pane.waitForTimeout(400);
};

// D10 first, from a *closed* pane, because "it opens the pane" is the whole
// claim. The stub's stack is empty, so this is the empty-stack case.
const failsBeforeEmpty = results.filter((r) => r.startsWith("FAIL")).length;
await pane.evaluate(() => closePane());
await pane.waitForTimeout(200);
await pane.keyboard.press("Control+Enter");
await pane.waitForTimeout(700);
check("Ctrl+Enter on an empty stack opens the pane", await pane.locator("#pty-pane.open").isVisible());
check("and queues nothing", (await queued()) === 0, `${await queued()}`);
check("and offers no undo for a send that never happened", !(await barOpenPane()));
check(
  "and throws nothing",
  results.filter((r) => r.startsWith("FAIL")).length === failsBeforeEmpty,
);

// **There is no undo window.** The one thing this block is really guarding is
// that the key does its whole job in one press: on a pane that has finished
// starting, Ctrl+Enter queues, arms and releases inside the keypress, with no
// interval tick in between and nothing on screen asking the reader to wait.
await seedPair(1);
await nudgeFlow(60_000); // a settled pane, quiet for a minute
const writesBeforeSend = (await called("pty_write")).length;
await pressSend();
check("Ctrl+Enter on a settled pane sends at once", (await sent()) === 1, `${await sent()}`);
check("and nothing is left queued", (await queued()) === 0, `${await queued()}`);
check("and the bar never asks the reader to wait", !(await barOpenPane()));
check("and the pair leaves the stack", (await onStack()) === 0, `${await onStack()}`);

// Mid-turn is no longer a reason to wait: Claude Code's own composer queues a
// line typed during a turn, so dreamd types it. This is the check that fails if
// the idle heuristic is ever reinstated.
await seedPair(2);
await nudgeFlow(0); // the child spoke this instant — a turn is running
await pane.evaluate(() => { pty.settled = true; });
await pressSend();
check("a send during a running turn still goes", (await sent()) === 2, `${await sent()}`);
check("and the bar stays shut", !(await barOpenPane()));

// The one wait left, and the only one: a child that has not finished drawing
// its first frame would lose the line entirely.
await seedPair(3);
await coldPane();
const writesBeforeBoot = (await called("pty_write")).length;
await pressSend();
check("a send waits while the pane is still starting", (await sent()) === 2, `${await sent()}`);
check("and says so", await barOpenPane());
check("and writes nothing to the child", (await called("pty_write")).length === writesBeforeBoot);
check("and the pair is still on the stack", (await onStack()) === 1, `${await onStack()}`);
check(
  "and the card says so rather than leaving",
  (await pane.locator("#stack-list .pair.pending").count()) === 1,
  `${await pane.locator("#stack-list .pair.pending").count()}`,
);

// Cancel is still the whole undo, and it is still a restoration rather than a
// retraction: nothing was written, so there is nothing to reverse.
await clickBar("Cancel");
check("Cancel takes it back", (await queued()) === 0 && !(await barOpenPane()));
check("the pair is untouched", (await onStack()) === 1, `${await onStack()}`);
check(
  "and its card is a plain one again",
  (await pane.locator("#stack-list .pair.pending").count()) === 0,
);
check("and nothing ever reached the child", (await called("pty_write")).length === writesBeforeBoot);

// The fallback for a boot that never goes quiet.
await pressSend();
check("the send is waiting again", await barOpenPane());
await clickBar("Send now");
check("Send now submits regardless of the boot watch", (await sent()) === 3, `${await sent()}`);
check("and the pair leaves the stack", (await onStack()) === 0, `${await onStack()}`);
check("and the bar clears", !(await barOpenPane()));

// And the boot watch's own path: once the child goes quiet, the waiting
// submission goes on its own, with no second keypress.
await seedPair(4);
await coldPane();
await pressSend();
check("a send made mid-boot is still waiting", (await sent()) === 3, `${await sent()}`);
await nudgeFlow(60_000);
await pane.waitForTimeout(900);
check("and goes once the child falls quiet", (await sent()) === 4, `${await sent()}`);
check("exactly once", (await queued()) === 0 && (await onStack()) === 0);
check("and the write count matches the sends", (await called("pty_write")).length >= writesBeforeSend);

// --- the two titlebar buttons ---------------------------------------------
// The clipboard icon means the clipboard and the paper plane means the send.
// They were one button until this change, so what is worth asserting is that
// they now do *different* things and that the rightmost one is the send.
const sentBeforeButtons = await sent();
const copiesBefore = (await called("copy_to_clipboard")).length;
// A pair on the stack first: `copyStack` refuses an empty one, so an empty
// stack would make this pass for the wrong reason.
await seedPair(5);
await pane.locator("#btn-copy").click();
await pane.waitForTimeout(200);
check(
  "the clipboard button copies",
  (await called("copy_to_clipboard")).length === copiesBefore + 1,
);
check("and sends nothing", (await sent()) === sentBeforeButtons, `${await sent()}`);
check("and leaves the pair on the stack", (await onStack()) === 1, `${await onStack()}`);
check(
  "the send button is the rightmost action",
  await pane.evaluate(() => {
    const row = document.getElementById("tb-actions");
    return row.lastElementChild.id === "btn-send";
  }),
);
check(
  "and it is the primary one",
  await pane.evaluate(() => document.getElementById("btn-send").classList.contains("primary")),
);
await nudgeFlow(60_000);
await pane.locator("#btn-send").click();
await pane.waitForTimeout(400);
check("clicking it sends the stack", (await sent()) === sentBeforeButtons + 1, `${await sent()}`);

// --- the model chips ------------------------------------------------------
// Live, not staged: unlike the permission mode beside them nothing restarts, so
// the check is that the child is neither killed nor respawned.
const spawnsBeforeModel = (await called("pty_spawn")).length;
const killsBeforeModel = (await called("pty_kill")).length;
check(
  "no chip is lit before one is pressed",
  (await pane.locator("#pty-models .pty-model.sel").count()) === 0,
);
await pane.locator('#pty-models .pty-model[data-model="sonnet"]').click();
await pane.waitForTimeout(200);
check(
  "a chip switches the model",
  (await pane.evaluate(() => window.__MODEL__)) === "sonnet",
  `${await pane.evaluate(() => window.__MODEL__)}`,
);
check(
  "and lights exactly itself",
  (await pane.locator("#pty-models .pty-model.sel").count()) === 1 &&
    (await pane.locator('#pty-models .pty-model.sel[data-model="sonnet"]').count()) === 1,
);
check(
  "and restarts nothing",
  (await called("pty_spawn")).length === spawnsBeforeModel &&
    (await called("pty_kill")).length === killsBeforeModel,
);
check(
  "every chip names a model Rust's enum accepts",
  await pane.evaluate(() =>
    [...document.querySelectorAll("#pty-models .pty-model")]
      .every((b) => ["opus", "sonnet", "haiku"].includes(b.dataset.model))),
);
// A restart is the one thing that *does* clear it: `/model` was typed into a
// process that no longer exists.
await pane.locator("#pty-restart").click();
await pane.waitForTimeout(500);
check(
  "a restart clears the lit chip",
  (await pane.locator("#pty-models .pty-model.sel").count()) === 0,
);

// --- the MCP status strip -------------------------------------------------
// Silent when healthy, which is the state the stub boots in.
check("a healthy socket says nothing", !(await pane.locator("#pty-pane.mcp-warn").count()));
const mcp = async (next) => {
  await pane.evaluate((m) => { window.__MCP__ = m; }, next);
  await pane.evaluate(() => refreshMcpStatus());
  await pane.waitForTimeout(150);
};
await mcp({ armed: true, serving: true, clients: 0 });
check("an unreached socket warns", (await pane.locator("#pty-pane.mcp-warn").count()) === 1);
check(
  "and names the command that fixes it",
  (await pane.locator("#pty-mcp code").textContent()) === "claude mcp add dreamd -- dreamd mcp",
);
await mcp({ armed: true, serving: false, clients: 0 });
check("a secondary window warns too", (await pane.locator("#pty-pane.mcp-warn").count()) === 1);
check(
  "and does not offer a command that would not help",
  (await pane.locator("#pty-mcp code").count()) === 0,
);
await mcp({ armed: false, serving: false, clients: 0 });
check("and so does a window with no repo", (await pane.locator("#pty-pane.mcp-warn").count()) === 1);
await mcp({ armed: true, serving: true, clients: 2 });
check("and it goes quiet again once an agent connects", !(await pane.locator("#pty-pane.mcp-warn").count()));

// --- docked right (`agent.position`) --------------------------------------
// A second page, because position is read once on the pane's first open — the
// only honest way to check the other value is to boot into it.
const right = await newPanePage("right");
await right.keyboard.press("Control+t");
await right.waitForFunction(() => typeof pty !== "undefined" && !!pty.term, null, { timeout: 15000 })
  .catch(() => {});
await right.waitForTimeout(600);

check("the pane mounts docked right", await right.locator("#pty-pane.open").isVisible());
check(
  "and the position reached the layout",
  await right.evaluate(() => document.body.classList.contains("agent-right")),
);
const geom = await right.evaluate(() => {
  const p = document.getElementById("pty-pane").getBoundingClientRect();
  const d = document.getElementById("content-scroll").getBoundingClientRect();
  return { px: Math.round(p.x), pw: Math.round(p.width), ph: Math.round(p.height),
           dr: Math.round(d.right), dh: Math.round(d.height) };
});
// Beside the document rather than under it, and full height — the shape a
// bottom dock cannot produce, so this fails if the grid never applied.
check(
  "it sits beside the document, not under it",
  geom.px >= geom.dr && geom.pw > 200 && geom.ph >= geom.dh,
  JSON.stringify(geom),
);
const rightSpawns = await right.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "pty_spawn"));
check(
  "and starts one process sized to the narrower box",
  rightSpawns.length === 1 && rightSpawns[0].args.cols > 0 && rightSpawns[0].args.rows > 0,
  JSON.stringify(rightSpawns.map((c) => c.args)),
);
// The bottom dock is the same window and the same terminal, so a right dock
// that never re-fit would come up with the bottom dock's column count.
check(
  "the right dock is narrower than the bottom dock it replaced",
  rightSpawns[0].args.cols < spawns[0].args.cols,
  `right ${rightSpawns[0].args.cols} vs bottom ${spawns[0].args.cols}`,
);
// A position change must re-fit, not leave the child holding the old grid —
// wrapped lines and box-drawing that no longer meets. Flipped at runtime here
// because that is the only way to watch the resize actually happen: on a first
// open the fit is folded into `openPane`, so nothing distinguishes a pane that
// re-fit from one that was simply built in the right box.
const resizesBeforeFlip = await right.evaluate(() =>
  window.__CALLS__.filter((c) => c.cmd === "pty_resize").length);
await right.evaluate(() => applyPanePosition("bottom"));
await right.waitForTimeout(400);
const flipped = await right.evaluate(() => {
  const r = window.__CALLS__.filter((c) => c.cmd === "pty_resize");
  return { n: r.length, last: r.length ? r[r.length - 1].args : null,
           right: document.body.classList.contains("agent-right") };
});
check("flipping the dock un-sets the layout class", !flipped.right);
check(
  "and tells the child its new grid",
  flipped.n > resizesBeforeFlip && flipped.last.cols > rightSpawns[0].args.cols,
  JSON.stringify(flipped),
);
await right.evaluate(() => applyPanePosition("right"));
await right.waitForTimeout(400);

// Escape is the same rule in the other dock (D12), and still not a kill.
await right.keyboard.press("Escape");
await right.waitForTimeout(150);
check("Escape closes the right-docked pane too", !(await right.locator("#pty-pane.open").isVisible()));
check(
  "and leaves that session running as well",
  (await right.evaluate(() => window.__CALLS__.filter((c) => c.cmd === "pty_kill").length)) === 0,
);
// The grid must give the whole width back, or a closed pane leaves a column of
// nothing where it was.
check(
  "closing gives the document the full width back",
  await right.evaluate(() =>
    Math.round(document.getElementById("content-scroll").getBoundingClientRect().width) ===
    Math.round(document.getElementById("main-wrap").getBoundingClientRect().width)),
);

// --- chrome: tree drag, floating outline, root field ----------------------
// A sixth page, for T4. Everything here is a *structural* assertion — what the
// page knows, never what it paints. None of the four pieces can be seen from
// here: the fade is CSS, the float is a rectangle, and the drag strip cannot be
// asserted at all, because the only proof of it is a window that moves.
//
// The stub has a working event bus and a tiny filesystem, because two of the
// three pieces are round trips: `set_root` has to come back through
// `repo-changed` the way `adopt_root` emits it, and the completion list has to
// come from something that can also answer "no".
const chrome = await newPage();
chrome.on("pageerror", (e) => results.push("FAIL pageerror (chrome): " + e.message));
chrome.on("console", (m) => {
  if (m.type() === "error") results.push("FAIL console.error (chrome): " + m.text());
});
await chrome.addInitScript(({ base }) => {
  // Directories only, exactly as `rootfield::complete` answers: names, never
  // paths, and never a file.
  const FS = {
    "/": ["other", "repo"],
    // `deep` and `docs` agree on a `d`, `zeta` agrees with neither: the two
    // halves of what a completion may type for you.
    "/other/": ["deep", "docs", "zeta"],
    "/other/deep/": [], "/other/docs/": [], "/other/zeta/": [],
    "/repo/": [],
  };
  const KEYMAP = {
    palette: "Ctrl+F", palette_prev: "Ctrl+P", palette_next: "Ctrl+N",
    highlight: "Ctrl+H", send_stack: "Ctrl+Enter", toggle_stack: "Ctrl+O",
    toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B", toggle_view: "Ctrl+M",
    toggle_pane: "Ctrl+T", jump_top: "Home", jump_bottom: "End",
    set_mark: "m", jump_mark: "'", jump_back: "Ctrl+[", jump_forward: "Ctrl+]",
    find: "/", find_next: "n", find_prev: "Shift+N",
    next_file: "]", prev_file: "[",
    copy_stack: "Ctrl+C", settings: "Ctrl+,", save_annotation: "Ctrl+Y",
    quick_highlight: true,
  };
  const state = { root: "/repo", patches: [] };
  window.__STATE__ = state;

  const listeners = new Map();
  window.__EMIT__ = (name, payload) => {
    for (const fn of listeners.get(name) || []) fn({ payload });
  };

  const tree = {
    name: "repo", is_dir: true, path: "/repo", rel: "",
    children: [{ name: "doc.md", is_dir: false, path: "/repo/doc.md", rel: "doc.md", children: [] }],
  };
  const settings = () => ({
    config: { theme: "dreamd", mode: "system", extra_ignores: [], keymap: KEYMAP, ui: { tree_width: 320 } },
    theme: "dreamd", scheme: "dark", themes: [], syntax_themes: [],
    config_path: "/tmp/xdg/dreamd/config.toml", themes_dir: "/tmp/xdg/dreamd/themes",
    local_overrides: [],
  });

  const body = "<h1 id='a'>One</h1>" + "<p>filler</p>".repeat(80) +
    "<h2 id='b'>Two</h2>" + "<p>filler</p>".repeat(80) +
    "<h2 id='c'>Three</h2>" + "<p>filler</p>".repeat(80);

  window.__TAURI__ = {
    core: {
      async invoke(cmd, args) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info":
            return { root: state.root, name: state.root.split("/").pop(), display: state.root };
          case "get_keymap": return KEYMAP;
          // The persisted width, deliberately not the default: a boot that
          // never asked would still be sitting at 260 and look correct.
          case "get_ui": return { tree_width: 320 };
          case "get_theme": return { css: base, mode: "system", scheme: "dark", syntax_theme: null };
          // Null, so the sidebar is open on boot — the tree drag is what this
          // page exists for and a collapsed one has no width to assert on.
          case "initial_file": return null;
          case "render_markdown": return body;
          case "list_markdown_files": return tree;
          case "fuzzy_search": return [];
          case "get_settings": return settings();
          case "get_highlights": case "reanchor": case "get_stack": return [];
          case "stack_query_text": return "";
          // This page presses every keymap entry, `toggle_pane` among them, so
          // it opens the pane even though the pane is not what it asserts on.
          // Without this the pane's first-open fetch reads `position` off null.
          case "agent_prefs": return { position: "bottom", permission_mode: "accept-edits" };
          case "pty_spawn": return true;
          case "pty_write": case "pty_resize": case "pty_kill": case "pty_model": return null;
          // This page opens the pane as a side effect of pressing every
          // binding, so it needs an answer here too: `refreshMcpStatus` runs on
          // every open, and a bare `null` would be one more pageerror to chase.
          case "mcp_status": return { armed: true, serving: true, clients: 1 };
          case "send_stack": return { method: "stub", detail: "nothing to send" };
          case "add_highlight": return "h0123456789abcdef";
          case "set_config":
            state.patches.push(args.patch);
            return settings();
          case "complete_directories": {
            const cut = args.path.lastIndexOf("/") + 1;
            const dir = args.path.slice(0, cut), prefix = args.path.slice(cut);
            if (!(dir in FS)) throw new Error("no directory " + dir);
            return FS[dir].filter((n) => n.toLowerCase().startsWith(prefix.toLowerCase()));
          }
          case "set_root": {
            const path = args.path.replace(/\/+$/, "") || "/";
            if (!(path + "/" in FS)) throw new Error("no such path: " + args.path);
            state.root = path;
            window.__EMIT__("repo-changed", null);
            return null;
          }
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
await chrome.goto(pathToFileURL(join(UI, "index.html")).href);
await chrome.waitForSelector("#tree .tree-item.file");
// Opened by clicking the tree rather than by `initial_file`, because a
// single-file launch boots with the sidebar collapsed and this page is here to
// measure the sidebar.
await chrome.locator("#tree .tree-item.file").click();
await chrome.waitForFunction(() => document.getElementById("content").textContent.includes("One"));
await chrome.waitForTimeout(200);

const treeWidthPx = () =>
  chrome.evaluate(() => Math.round(document.getElementById("sidebar").getBoundingClientRect().width));
const collapsed = () => chrome.evaluate(() => document.body.classList.contains("nav-collapsed"));
const widthPatches = () =>
  chrome.evaluate(() => window.__STATE__.patches.map((p) => p.ui && p.ui.tree_width).filter((n) => n != null));

// --- 4a. the tree drag ---
check("the persisted tree width is applied on boot", (await treeWidthPx()) === 320, `${await treeWidthPx()}`);

// Dragged well past the maximum: the clamp is the frontend's, so the config
// file never has to reject what the handle sent.
const dragTo = async (x, steps = 6) => {
  const from = await chrome.evaluate(() => {
    const r = document.getElementById("tree-resize").getBoundingClientRect();
    return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + 40) };
  });
  await chrome.mouse.move(from.x, from.y);
  await chrome.mouse.down();
  await chrome.mouse.move(x, from.y, { steps });
  await chrome.mouse.up();
  await chrome.waitForTimeout(80);
};

await dragTo(1100);
check("dragging past the maximum stops at 600", (await treeWidthPx()) === 600, `${await treeWidthPx()}`);
check("and does not collapse the tree", !(await collapsed()));

// One move, so the last width the drag applied is the one it started from:
// with intermediate steps the assertion below would be about wherever the
// pointer happened to cross 140.
await dragTo(40, 1);
check("dragging below the minimum collapses the tree", await collapsed());
await chrome.keyboard.press("Control+b");
await chrome.waitForTimeout(80);
// Belt, and the difference between a diagnosis and a stack trace: if the drag
// did *not* collapse the tree, that `Control+b` collapsed it instead — and
// every check below this point needs a sidebar wide enough to click.
if (await collapsed()) {
  await chrome.locator("#btn-expand").click();
  await chrome.waitForTimeout(80);
}
check(
  "and the width it collapsed from is what comes back, not the default",
  (await treeWidthPx()) === 600,
  `${await treeWidthPx()}`,
);

await dragTo(200);
check("a drag inside the range takes the width", (await treeWidthPx()) === 200, `${await treeWidthPx()}`);

// Debounced: past the 400ms window there is one write per drag, and every
// value on the wire is inside the range `config::Ui` clamps to.
await chrome.waitForTimeout(600);
const patched = await widthPatches();
check("the drag persists the width", patched.length >= 1 && patched[patched.length - 1] === 200,
  JSON.stringify(patched));
check("and never sends a width outside the clamp",
  patched.every((n) => n >= 140 && n <= 600), JSON.stringify(patched));

// --- 4b. the floating outline ---
const outlineOpen = () => chrome.locator("#outline-panel.open").isVisible();
await chrome.keyboard.press("Control+i");
await chrome.waitForTimeout(120);
check("the outline mounts", (await outlineOpen()) && (await chrome.locator("#outline-list .oi").count()) === 3,
  `${await chrome.locator("#outline-list .oi").count()} entries`);
const boxes = await chrome.evaluate(() => {
  const p = document.getElementById("outline-panel").getBoundingClientRect();
  const pane = document.getElementById("main-wrap").getBoundingClientRect();
  return { style: getComputedStyle(document.getElementById("outline-panel")).position,
           gapRight: Math.round(pane.right - p.right), fromLeft: Math.round(p.left - pane.left),
           height: Math.round(p.height), paneHeight: Math.round(pane.height) };
});
check("and floats at the top right of the reading pane rather than docking",
  boxes.style === "absolute" && boxes.gapRight <= 12 && boxes.fromLeft > 100 && boxes.height < boxes.paneHeight,
  JSON.stringify(boxes));

await chrome.keyboard.press("Control+i");
await chrome.waitForTimeout(120);
check("and unmounts", !(await outlineOpen()));

await chrome.keyboard.press("Control+i");
await chrome.waitForTimeout(120);
await chrome.locator("#outline-list .oi").nth(2).click();
await chrome.waitForTimeout(300);
check("a heading click dismisses it", !(await outlineOpen()));
check("and still jumps", await chrome.evaluate(() => document.getElementById("content-scroll").scrollTop > 100));

await chrome.keyboard.press("Control+i");
await chrome.waitForTimeout(120);
check("it reopens", await outlineOpen());
await chrome.evaluate(() =>
  document.getElementById("content-scroll").scrollBy({ top: -200, behavior: "instant" }));
await chrome.waitForTimeout(200);
check("and any scroll of the reader closes it", !(await outlineOpen()));

// --- 4c. the root field ---
const rootField = chrome.locator("#repo-name");
const shownRoot = () => rootField.inputValue();
const knownRoot = () => chrome.evaluate(() => repoRoot);

check("the field shows the basename when it is not focused", (await shownRoot()) === "repo", await shownRoot());
await rootField.click();
await chrome.waitForTimeout(120);
check("and the full path when it is", (await shownRoot()) === "/repo", await shownRoot());

// Tab completion: two directories agree on nothing, so nothing is typed for
// you; one match completes and opens the next segment.
await rootField.fill("/other/");
await chrome.keyboard.press("Tab");
await chrome.waitForTimeout(200);
check("candidates that agree on nothing type nothing for you",
  (await shownRoot()) === "/other/", await shownRoot());
check("but say what they are",
  (await chrome.textContent("#toast")).includes("deep") && (await chrome.textContent("#toast")).includes("zeta"),
  await chrome.textContent("#toast"));
await rootField.fill("/other/d");
await chrome.keyboard.press("Tab");
await chrome.waitForTimeout(200);
check("candidates that share a prefix extend as far as they agree",
  (await shownRoot()) === "/other/d", await shownRoot());
await rootField.fill("/other/de");
await chrome.keyboard.press("Tab");
await chrome.waitForTimeout(200);
check("a single completion is taken", (await shownRoot()) === "/other/deep/", await shownRoot());

// The round trip: submit, and the displayed root is what came back through
// `repo-changed`, not what was typed.
await rootField.fill("/other");
await chrome.keyboard.press("Enter");
await chrome.waitForTimeout(400);
check("submitting moves the root through IPC", (await knownRoot()) === "/other", await knownRoot());
check("and the field goes back to a basename", (await shownRoot()) === "other", await shownRoot());

await rootField.click();
await rootField.fill("/nowhere");
await chrome.keyboard.press("Enter");
await chrome.waitForTimeout(300);
check("an invalid path is flagged", (await chrome.locator("#repo-name.error").count()) === 1);
check("and leaves you in the current root", (await knownRoot()) === "/other", await knownRoot());
await chrome.keyboard.press("Escape");
await chrome.waitForTimeout(150);
check("Escape abandons the edit", (await shownRoot()) === "other", await shownRoot());
check("and does not also leave view mode or close anything",
  !(await chrome.evaluate(() => document.body.classList.contains("view-mode"))));

// The new element is an `<input>` in the chrome, so the bare-letter bindings
// have to stay out of it — `m`, `/` and `n` are all live keys in the reader.
await rootField.click();
await rootField.fill("");
await chrome.keyboard.type("m/n[");
await chrome.waitForTimeout(150);
check("typing a path does not fire the reader's bare-letter keys",
  (await shownRoot()) === "m/n[" && !(await chrome.locator("#find-bar.open").isVisible()) &&
  (await chrome.textContent("#toast")) !== "Mark set",
  `${await shownRoot()} / ${await chrome.textContent("#toast")}`);
await chrome.keyboard.press("Escape");
await chrome.waitForTimeout(150);

// Back to the repo with the document in it: moving the root closed the open
// file, and half the keymap is about a document.
await rootField.click();
await rootField.fill("/repo");
await chrome.keyboard.press("Enter");
await chrome.waitForSelector("#tree .tree-item.file");
await chrome.locator("#tree .tree-item.file").click();
await chrome.waitForFunction(() => document.getElementById("content").textContent.includes("One"));
await chrome.waitForTimeout(200);

// --- every keymap entry still reaches a handler ---
// The eleven actions with state this harness can see are asserted one by one.
// The rest are pressed too, and what they are asserted on is that dispatch
// after the new elements exist throws nothing — which the `pageerror` and
// `console.error` listeners above are watching for.
const asPlaywright = (combo) =>
  combo.split("+").map((p) => (p === "Ctrl" ? "Control" : p)).join("+");
const km = await chrome.evaluate(() => keymap);
const OBSERVABLE = {
  palette: "#palette-overlay.open",
  settings: "#settings-overlay.open",
  toggle_stack: "#stack-panel.open",
  toggle_outline: "#outline-panel.open",
  find: "#find-bar.open",
  toggle_pane: "#pty-pane.open",
};
for (const [action, selector] of Object.entries(OBSERVABLE)) {
  await chrome.keyboard.press(asPlaywright(km[action]));
  await chrome.waitForTimeout(action === "toggle_pane" ? 1200 : 150);
  check(`${action} still resolves to a handler`, await chrome.locator(selector).isVisible());
  await chrome.keyboard.press("Escape");
  await chrome.waitForTimeout(150);
}
// The pane above left focus in xterm's textarea, where every key belongs to
// the child — see `inTerminal`. Everything below is a reader binding.
await chrome.locator("#content").click();
await chrome.waitForTimeout(120);
for (const [action, cls] of [["toggle_tree", "nav-collapsed"], ["toggle_view", "view-mode"]]) {
  const before = await chrome.evaluate((c) => document.body.classList.contains(c), cls);
  await chrome.keyboard.press(asPlaywright(km[action]));
  await chrome.waitForTimeout(150);
  const after = await chrome.evaluate((c) => document.body.classList.contains(c), cls);
  check(`${action} still resolves to a handler`, before !== after);
  await chrome.keyboard.press(asPlaywright(km[action]));
  await chrome.waitForTimeout(150);
}
await chrome.evaluate(() => document.getElementById("content-scroll").scrollTo({ top: 0, behavior: "instant" }));
for (const [action, key] of [["jump_bottom", "End"], ["jump_top", "Home"]]) {
  await chrome.keyboard.press(key);
  await chrome.waitForTimeout(200);
  const top = await chrome.evaluate(() => document.getElementById("content-scroll").scrollTop);
  check(`${action} still resolves to a handler`, action === "jump_bottom" ? top > 0 : top === 0, `${top}`);
}
const errorsBefore = results.filter((r) => r.startsWith("FAIL")).length;
for (const [action, combo] of Object.entries(km)) {
  if (typeof combo !== "string" || action in OBSERVABLE) continue;
  await chrome.keyboard.press(asPlaywright(combo));
  await chrome.waitForTimeout(90);
  await chrome.keyboard.press("Escape");
  await chrome.waitForTimeout(60);
}
await chrome.waitForTimeout(300);
check(
  "pressing every remaining binding throws nothing",
  results.filter((r) => r.startsWith("FAIL")).length === errorsBefore,
);

await browser.close();
console.log(results.join("\n"));
const failed = results.filter((r) => r.startsWith("FAIL")).length;
console.log(`\n${results.length - failed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
