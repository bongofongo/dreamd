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
import { pathToFileURL } from "node:url";
import { join } from "node:path";
import { readFileSync, readdirSync } from "node:fs";

const UI = "/Users/oliverfong/toadmountain/dreamd/ui";
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
const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
page.on("pageerror", (e) => results.push("FAIL pageerror: " + e.message));
page.on("console", (m) => {
  if (m.type() === "error") results.push("FAIL console.error: " + m.text());
});

await page.addInitScript(({ base, palettes }) => {
  const KEYMAP = {
    palette: "Ctrl+F", palette_prev: "Ctrl+P", palette_next: "Ctrl+N",
    highlight: "Ctrl+H", send_stack: "Ctrl+Enter", toggle_stack: "Ctrl+O",
    copy_stack: "Ctrl+C", settings: "Ctrl+,", save_annotation: "Ctrl+Y",
    quick_highlight: true,
  };
  const state = {
    config: { theme: "dreamd", tmux_autodetect: true, extra_ignores: [], keymap: { ...KEYMAP } },
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

  const settings = () => ({
    config: JSON.parse(JSON.stringify(state.config)),
    theme: state.config.theme,
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
          case "get_theme_css": return cssFor(state.config.theme);
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

// --- the theme actually applies (this is the CSS split working) ---
check("base+palette applied on boot", (await varOf("--bg")) === "#1b1f27", await varOf("--bg"));
check(
  "content width comes from the palette",
  (await page.evaluate(() => getComputedStyle(document.getElementById("content")).maxWidth)) === "820px",
);

// --- open via the keybind ---
await page.keyboard.press("Control+Comma");
check("Ctrl+, opens settings", await page.locator("#settings-overlay.open").isVisible());

// --- keys tab ---
const rows = await page.locator("#st-keys .st-row").count();
check("every action gets a row", rows === 10, `got ${rows}`);
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
await page.locator("#st-theme-grid .th-card", { hasText: "gruvbox-dark" }).first()
  .locator("button", { hasText: "Duplicate" }).click();
await page.waitForTimeout(250);
check("duplicate switches to the custom tab", await page.locator("#st-pane-custom.sel").isVisible());
const varRows = await page.locator("#st-vars .st-var").count();
check("var editor lists the palette", varRows === 20, `got ${varRows}`);
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

// --- closing ---
await page.keyboard.press("Escape");
check("Esc closes the panel", !(await page.locator("#settings-overlay.open").isVisible()));

await browser.close();
console.log(results.join("\n"));
const failed = results.filter((r) => r.startsWith("FAIL")).length;
console.log(`\n${results.length - failed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
