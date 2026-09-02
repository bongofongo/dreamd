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
    toggle_mode: "Ctrl+Shift+D",
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
      // `config::Ui` is a plain struct, so Rust always sends all eight —
      // spelling them out here is what keeps the Window tab's checks below
      // asserting on the payload the real backend produces. `titlebar_fade`
      // is the macOS default rather than this runner's, so the class it drives
      // is exercised on both.
      ui: {
        tree_width: 260, stack_width: 280, pane_width: 380, pane_height: 240,
        menubar: false, titlebar: false, titlebar_fade: true, zoom: 100,
      },
      // `config::Agent` is a plain struct too, and the Window tab now renders
      // two of its fields. Spelled out for the same reason `ui` is.
      agent: {
        position: "right", permission_mode: "accept-edits",
        surface: "native", popout: "never",
      },
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
    // The *OS* appearance, which is what `state.scheme` has always held here:
    // `set_appearance` is the only thing that writes it, and the frontend only
    // pushes that under `mode = "system"`. Pinning light or dark moves
    // `scheme` above and must leave this one alone — the System button's label
    // is the assertion.
    system: state.scheme,
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

  window.__PRINTS__ = 0;
  window.__TAURI__ = {
    core: {
      // Tauri's own, and the only URL a local image ever gets: the CSP
      // admits `asset:` and not `file:`. The test image resolves to a
      // data URI so Chromium can actually decode one; every other path
      // comes back in the real shape, and `__ASSET__` records what the
      // containment guard handed over.
      convertFileSrc(path) {
        (window.__ASSET__ ||= []).push(path);
        return path.endsWith("pic.png")
          ? window.__PNG__
          : "asset://localhost/" + encodeURIComponent(path);
      },
      async invoke(cmd, args) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return state.config.keymap;
          // The boot's second answer, off the same table `set_config` merges
          // into — so the Window tab's checks below start from the state the
          // page actually booted with rather than from `default: null`, which
          // is `applyWindowChrome`'s "assume the platform default" branch and
          // would have disagreed with the payload.
          case "get_ui": return state.config.ui;
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
          // The settings panel's one *action*. Counted rather than recorded
          // because this stub keeps no call log — the pane's does, and this page
          // has one thing worth counting.
          case "print_document": window.__PRINTS__++; return null;
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

// --- the agent composer reads the palette, not a fallback ---
// Asserted against `body`'s colour rather than a hex, because the claim is
// "the same variable the document uses" and that survives a palette edit.
// It is worth a check at all because the failure was silent for both the
// engine and the eye in dark mode: `#agent-input` read `var(--fg, #e6e1f2)`,
// no palette in `ui/themes/` has ever declared `--fg`, so the fallback won in
// every theme and the composer painted near-white text on the light half of
// every family. Both appearances are checked — a value pinned to the dark
// fallback passes any dark-only assertion by accident.
const composerTracksPalette = () =>
  page.evaluate(() => {
    const input = getComputedStyle(document.getElementById("agent-input"));
    const body = getComputedStyle(document.body);
    return { input: input.color, body: body.color };
  });
for (const mode of ["dark", "light"]) {
  await page.evaluate((m) => document.documentElement.setAttribute("data-mode", m), mode);
  const { input, body } = await composerTracksPalette();
  check(`composer text is the palette's --text (${mode})`, input === body, `${input} vs ${body}`);
}
await page.evaluate(() => document.documentElement.setAttribute("data-mode", "dark"));

// --- open via the keybind ---
await page.keyboard.press("Control+Comma");
check("Ctrl+, opens settings", await page.locator("#settings-overlay.open").isVisible());

// --- keys tab ---
// Counted against the page's own KEY_ACTIONS rather than a literal: a hardcoded
// 25 said nothing about the list once it grew past 25, and the row count also
// carries the mode picker and the quick-highlight checkbox, which are not
// actions. `KEY_ACTIONS` is a top-level `const` in a classic script, so it is
// in the page's global lexical scope and readable by name here.
const want = await page.evaluate(() => KEY_ACTIONS.length);
const rows = await page.locator("#st-keys .st-row[data-action]").count();
check("every action gets a row", rows === want, `got ${rows}, want ${want}`);
check(
  "a repo-shadowed key is flagged",
  (await page.locator("#st-keys .shadowed").count()) === 1,
);
// By action, not by position: the mode picker is the first `.st-row` and has no
// combo at all.
const paletteCombo = page.locator('#st-keys .st-row[data-action="palette"] button.combo');
const shown = (await paletteCombo.textContent()).trim();
check("combo is rendered", shown === "\u2303F" || shown === "Ctrl+F", shown);

// record a new binding for the palette
await paletteCombo.click();
check("recording state shown", (await paletteCombo.textContent()).includes("Press keys"));
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
await paletteCombo.click();
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
// Asserted by *label*, not by counting rows. A count was the original check and
// it silently became meaningless the moment the tab grew a third row for the
// agent surface: on macOS the menubar row is absent, so titlebar + surface came
// to 2 and the check passed for entirely the wrong reason — while on Linux the
// same three rows would have failed it. Naming what must be there says the
// thing the count was trying to say and cannot drift with the tab's length.
const chromeLabels = await page.evaluate(() =>
  [...document.querySelectorAll("#st-window .st-row .lbl")].map((l) => l.childNodes[0].textContent.trim()),
);
const isMacPage = await page.evaluate(() => document.body.classList.contains("mac"));
// Two rows that are each dead on the other platform, asserted in both
// directions — a one-way check would pass on the runner and be silent about the
// machine this was written on. The titlebar row is absent on macOS because
// there is no window-manager bar there to toggle (`chrome::set_titlebar`), and
// the fade row is absent off it because it is `backdrop-filter` over dreamd's
// own bar.
check(
  "the titlebar toggle is listed exactly off macOS",
  chromeLabels.includes("Native titlebar") === !isMacPage,
  `mac=${isMacPage} got ${JSON.stringify(chromeLabels)}`,
);
check(
  "and the fading bar exactly on it",
  chromeLabels.includes("Fade the top bar") === isMacPage,
  `mac=${isMacPage} got ${JSON.stringify(chromeLabels)}`,
);
check(
  "the menubar toggle is listed exactly off macOS",
  chromeLabels.includes("Native menubar") === !isMacPage,
  `mac=${isMacPage} got ${JSON.stringify(chromeLabels)}`,
);
check(
  "and the agent surface is a window-level choice",
  chromeLabels.includes("Terminal agent pane"),
  `got ${JSON.stringify(chromeLabels)}`,
);
check(
  "and so is where the conversation is drawn",
  chromeLabels.includes("Pop-out agent"),
  `got ${JSON.stringify(chromeLabels)}`,
);
// The select's round trip. Its *effect* is checked on the pane pages below,
// against a page booted into each value; this is only that the control writes
// the key those pages read.
const popSelect = page.locator("#st-window .st-row", { hasText: "Pop-out agent" }).locator("select");
check("the pop-out starts from the config payload", (await popSelect.inputValue()) === "never");
await popSelect.selectOption("send");
await page.waitForTimeout(200);
check(
  "choosing one writes agent.popout",
  (await page.evaluate(() => window.__STATE__.config.agent.popout)) === "send",
);
// Native-only, and the row says so by going dead rather than by disappearing —
// a control that vanished would leave a reader who turned the terminal on
// hunting for a setting that was there a moment ago.
const surfaceBox = page.locator("#st-window .st-row", { hasText: "Terminal agent pane" })
  .locator('input[type="checkbox"]');
await surfaceBox.click();
await page.waitForTimeout(200);
check("the terminal fallback disables the pop-out row", await popSelect.isDisabled());
await surfaceBox.click();
await page.waitForTimeout(200);
check("and turning it back off re-enables it", !(await popSelect.isDisabled()));
await popSelect.selectOption("never");
await page.waitForTimeout(150);
// `ui.titlebar_fade` is the one window setting the *page* applies rather than
// the native window, so it is the one this harness can see end to end. The class
// is toggled on every platform — only the CSS is scoped to `body.mac` — so the
// boot mapping is assertable on the runner as well as on a Mac.
const fadeClass = () => page.evaluate(() => document.body.classList.contains("chrome-fade"));
check("a titlebar_fade config paints the fading bar", await fadeClass());

// The row's own round trip. Which row that is depends on the platform — the two
// chrome toggles are each absent on the other one — so the key and the label are
// picked together, and a mismatch between them is what this would catch.
const [chromeLabel, chromeKey] = isMacPage
  ? ["Fade the top bar", "titlebar_fade"]
  : ["Native titlebar", "titlebar"];
const chromeBox = page.locator("#st-window .st-row", { hasText: chromeLabel })
  .locator('input[type="checkbox"]');
check(
  `${chromeKey} starts from the config payload`,
  (await chromeBox.isChecked()) === isMacPage,
);
await chromeBox.click();
await page.waitForTimeout(200);
check(
  `toggling writes ui.${chromeKey}`,
  (await page.evaluate((k) => window.__STATE__.config.ui[k], chromeKey)) === !isMacPage,
);
check("and the box holds its new state", (await chromeBox.isChecked()) === !isMacPage);
if (isMacPage) {
  // Only reachable where the row exists, and the half worth having: the fade is
  // a CSS mode, so turning it off has to take the class with it.
  check("and turning the fade off drops the class", !(await fadeClass()));
  await chromeBox.click();
  await page.waitForTimeout(200);
  check("and back on restores it", await fadeClass());
}

// Print, the tab's one action rather than a preference — it moved off the
// titlebar and this row is now the only way to reach it. Chromium cannot open an
// OS print dialog, so what is checked is that the click reaches `print_document`
// and that the panel gets out of the way first.
check(
  "print is listed here now that it has left the titlebar",
  chromeLabels.includes("Print or save as PDF"),
  `got ${JSON.stringify(chromeLabels)}`,
);
// A file has to be open or `printDocument` refuses before it reaches IPC, which
// is a check of its own: nothing is stubbed here, so this page has opened
// nothing.
const printRow = page.locator("#st-window .st-row", { hasText: "Print or save as PDF" })
  .locator("button");
await printRow.click();
await page.waitForTimeout(200);
check(
  "printing with nothing open reaches no IPC",
  (await page.evaluate(() => window.__PRINTS__)) === 0,
);
check(
  "and leaves the panel up rather than closing it to say so",
  await page.locator("#settings-overlay.open").isVisible(),
);
await page.evaluate(() => { currentFile = "/repo/doc.md"; });
await printRow.click();
await page.waitForTimeout(200);
check(
  "and with one open it asks Rust to print",
  (await page.evaluate(() => window.__PRINTS__)) === 1,
);
check("and closes settings first", !(await page.locator("#settings-overlay.open").isVisible()));
await page.locator("#btn-settings").click();
await page.waitForTimeout(150);
await page.locator('.st-tab[data-pane="window"]').click();
await page.waitForTimeout(120);

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
// The System button answers for the *machine*, not for the palette pinned over
// it. It used to be labelled from the resolved scheme, so pressing light here
// relabelled it "System (light)" and going back to system then resolved against
// that — a preference that quietly changed meaning every time the other one was
// tried. The page is opened `colorScheme: "dark"`, so this must still say dark.
check(
  "System still names the OS appearance under a pinned light",
  (await page.locator("#st-mode .st-mode-btn", { hasText: "System" }).first().textContent())
    .toLowerCase().includes("dark"),
  await page.locator("#st-mode .st-mode-btn", { hasText: "System" }).first().textContent(),
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

// --- the appearance keybind ---
// The panel is shut, which is the state it is for: the same `setMode` the three
// buttons call, reached without opening anything. Click into the document first
// — the Custom tab left focus in a text input, and `isEditable` disarms every
// bare-modifier binding while one holds it.
await page.locator("#content").click();
await page.waitForTimeout(120);
const modeBefore = await page.evaluate(() => document.documentElement.dataset.mode);
await page.keyboard.press("Control+Shift+D");
await page.waitForTimeout(250);
check(
  "toggle_mode flips the appearance",
  (await page.evaluate(() => document.documentElement.dataset.mode)) !== modeBefore,
);
check(
  "and writes it to the config rather than only painting it",
  (await page.evaluate(() => window.__STATE__.config.mode)) ===
    (modeBefore === "dark" ? "light" : "dark"),
  await page.evaluate(() => window.__STATE__.config.mode),
);
await page.keyboard.press("Control+Shift+D");
await page.waitForTimeout(250);
check(
  "and back",
  (await page.evaluate(() => document.documentElement.dataset.mode)) === modeBefore,
);

// --- document zoom ---
// The whole feature is one custom property and two inline `calc()`s, so this is
// what there is to assert: that the keys move it, that the chrome does *not*
// move with it, and that the number reaches the config. The gestures cannot be
// checked here — WebKit's `gesture*` events do not exist in Chromium — but
// Ctrl+wheel is the same code path and is.
//
// `accel` rather than a hardcoded Control: `zoomKey` claims ⌘ on a page whose
// UA says Macintosh, which is what a developer running this locally has.
const accel = isMacPage ? "Meta" : "Control";
const zoomVar = () => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--zoom").trim());
const zoomShown = () => page.locator("#zoom-pct").textContent();

check("the document starts unzoomed", (await zoomVar()) === "1", await zoomVar());
check(
  "and the size that scales is inline on #content, where no palette can outrank it",
  await page.evaluate(() =>
    document.getElementById("content").style.fontSize.includes("--zoom") &&
    document.getElementById("content").style.maxWidth.includes("--zoom")),
);

await page.keyboard.press(`${accel}+=`);
await page.waitForTimeout(80);
check("a zoom-in key steps the ladder", (await zoomVar()) === "1.1", await zoomVar());
check("and the readout says so", (await zoomShown()) === "110%", await zoomShown());
check("and the pill is on screen", await page.locator("#zoom-pill.show").isVisible());

await page.keyboard.press(`${accel}+=`);
await page.keyboard.press(`${accel}+=`);
await page.waitForTimeout(80);
check("three steps land on 150%", (await zoomShown()) === "150%", await zoomShown());
check(
  "the chrome does not scale with the document",
  await page.evaluate(() => {
    const tree = getComputedStyle(document.getElementById("tree")).fontSize;
    return tree === getComputedStyle(document.body).fontSize;
  }),
);
// The config write is debounced by 400ms — the whole point of the debounce is
// that a pinch is one write, so this is also the assertion that three steps did
// not become three round trips.
await page.waitForTimeout(600);
check(
  "the zoom persists to config",
  (await page.evaluate(() => window.__STATE__.config.ui.zoom)) === 150,
  String(await page.evaluate(() => window.__STATE__.config.ui.zoom)),
);

// Ctrl+wheel: the mouse's spelling of a pinch, and the one gesture Chromium can
// be made to send. Over the document, and it must not scroll it.
const scrolledBefore = await page.evaluate(() => document.getElementById("content-scroll").scrollTop);
await page.mouse.move(600, 400);
await page.keyboard.down("Control");
await page.mouse.wheel(0, -120);
await page.keyboard.up("Control");
await page.waitForTimeout(120);
check(
  "Ctrl+wheel zooms rather than scrolls",
  (await zoomVar()) !== "1.5" &&
    (await page.evaluate(() => document.getElementById("content-scroll").scrollTop)) === scrolledBefore,
  await zoomVar(),
);

await page.keyboard.press(`${accel}+0`);
await page.waitForTimeout(80);
check("and 0 resets", (await zoomVar()) === "1", await zoomVar());

// Clamped at both ends, from the keyboard, so a reader leaning on the key
// cannot send the config something Rust has to clamp back.
for (let i = 0; i < 14; i++) await page.keyboard.press(`${accel}+-`);
await page.waitForTimeout(80);
check("the ladder stops at the floor", (await zoomShown()) === "50%", await zoomShown());
for (let i = 0; i < 20; i++) await page.keyboard.press(`${accel}+=`);
await page.waitForTimeout(80);
check("and at the ceiling", (await zoomShown()) === "300%", await zoomShown());

// The settings row is the same control seen twice: it reads what the gestures
// left and writes back through the same clamp.
await page.keyboard.press("Control+,");
await page.waitForSelector("#settings-overlay.open");
await page.locator('.st-tab[data-pane="window"]').click();
const zoomField = page.locator("#st-window .st-row", { hasText: "Zoom" }).locator("input");
await zoomField.fill("140");
await zoomField.press("Enter");
await page.waitForTimeout(200);
check("the settings field zooms the document", (await zoomVar()) === "1.4", await zoomVar());
check(
  "and writes the number it applied",
  (await page.evaluate(() => window.__STATE__.config.ui.zoom)) === 140,
  String(await page.evaluate(() => window.__STATE__.config.ui.zoom)),
);
await zoomField.fill("900");
await zoomField.press("Enter");
await page.waitForTimeout(200);
check("an out-of-range number snaps back in the field", (await zoomField.inputValue()) === "300");
// Claimed above the overlay guard on purpose: a panel you cannot read is the
// case for zooming, not a reason to be denied it.
await page.keyboard.press(`${accel}+0`);
await page.waitForTimeout(80);
check("the zoom keys work with the panel open", (await zoomVar()) === "1", await zoomVar());
await page.keyboard.press("Escape");
await page.waitForTimeout(150);

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
      // Tauri's own, and the only URL a local image ever gets: the CSP
      // admits `asset:` and not `file:`. The test image resolves to a
      // data URI so Chromium can actually decode one; every other path
      // comes back in the real shape, and `__ASSET__` records what the
      // containment guard handed over.
      convertFileSrc(path) {
        (window.__ASSET__ ||= []).push(path);
        return path.endsWith("pic.png")
          ? window.__PNG__
          : "asset://localhost/" + encodeURIComponent(path);
      },
      async invoke(cmd) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return {
            palette: "Ctrl+F", palette_prev: "Ctrl+P", palette_next: "Ctrl+N",
            highlight: "Ctrl+H", send_stack: "Ctrl+Enter", toggle_stack: "Ctrl+O",
            toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B", toggle_view: "Ctrl+M",
            toggle_mode: "Ctrl+Shift+D",
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
      // Tauri's own, and the only URL a local image ever gets: the CSP
      // admits `asset:` and not `file:`. The test image resolves to a
      // data URI so Chromium can actually decode one; every other path
      // comes back in the real shape, and `__ASSET__` records what the
      // containment guard handed over.
      convertFileSrc(path) {
        (window.__ASSET__ ||= []).push(path);
        return path.endsWith("pic.png")
          ? window.__PNG__
          : "asset://localhost/" + encodeURIComponent(path);
      },
      async invoke(cmd, args) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return {
            palette: "Ctrl+F", palette_prev: "Ctrl+P", palette_next: "Ctrl+N",
            highlight: "Ctrl+H", send_stack: "Ctrl+Enter", toggle_stack: "Ctrl+O",
            toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B", toggle_view: "Ctrl+M",
            toggle_mode: "Ctrl+Shift+D",
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
      // Tauri's own, and the only URL a local image ever gets: the CSP
      // admits `asset:` and not `file:`. The test image resolves to a
      // data URI so Chromium can actually decode one; every other path
      // comes back in the real shape, and `__ASSET__` records what the
      // containment guard handed over.
      convertFileSrc(path) {
        (window.__ASSET__ ||= []).push(path);
        return path.endsWith("pic.png")
          ? window.__PNG__
          : "asset://localhost/" + encodeURIComponent(path);
      },
      async invoke(cmd, args) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return {
            palette: "Ctrl+F", palette_prev: "Ctrl+P", palette_next: "Ctrl+N",
            highlight: "Ctrl+H", send_stack: "Ctrl+Enter", toggle_stack: "Ctrl+O",
            toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B", toggle_view: "Ctrl+M",
            toggle_mode: "Ctrl+Shift+D",
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
const paneStub = ({ base, position, surface, popout }) => {
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
  //
  // `surface` is settable per page because the pane has two bodies and the
  // checks below are split between them: the fit-addon geometry belongs to the
  // terminal, which is now the *fallback* surface, and the conversation log
  // belongs to the native one, which is the default. A page that wants the
  // terminal has to say so, exactly as a reader would.
  window.__AGENT__ = { position, permission_mode: "accept-edits", surface, popout };
  // What `mcp_status` answers. Healthy, so the strip is hidden on boot and the
  // checks that want it have to say what is wrong. `clients` is deliberately
  // absent: the strip stopped reading it, because a count of zero is equally
  // true of a correctly-wired agent that has not called a dreamd tool yet.
  window.__MCP__ = {
    armed: true, serving: true, registered: "yes",
    command: "claude mcp add dreamd --scope user -- /usr/bin/dreamd mcp",
  };
  window.__MODEL__ = null;
  /// What `agent_decide` was called with, so a card's three buttons can be told
  /// apart by what they *sent* rather than by what they look like.
  window.__DECIDED__ = [];
  const listeners = new Map();
  window.__EMIT__ = (name, payload) => {
    for (const fn of listeners.get(name) || []) fn({ payload });
  };
  window.__TAURI__ = {
    core: {
      // Tauri's own, and the only URL a local image ever gets: the CSP
      // admits `asset:` and not `file:`. The test image resolves to a
      // data URI so Chromium can actually decode one; every other path
      // comes back in the real shape, and `__ASSET__` records what the
      // containment guard handed over.
      convertFileSrc(path) {
        (window.__ASSET__ ||= []).push(path);
        return path.endsWith("pic.png")
          ? window.__PNG__
          : "asset://localhost/" + encodeURIComponent(path);
      },
      async invoke(cmd, args) {
        calls.push({ cmd, args });
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return {
            palette: "Ctrl+F", highlight: "Ctrl+H", toggle_stack: "Ctrl+O",
            toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B", toggle_view: "Ctrl+M",
            toggle_mode: "Ctrl+Shift+D",
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
          // The native surface. No child either: `agent/wire.rs` owns turning
          // Claude Code's JSON into events, and this page owns turning events
          // into DOM. The seam between them is `__EMIT__("agent-event", …)`,
          // which is exactly what `main.rs` emits.
          case "agent_spawn":
            // The one failure worth stubbing: `claude` not on the login shell's
            // PATH is what `agent::claude::resolve` refuses on, and it is the
            // thing a headerless pop-out has nowhere to say.
            if (window.__SPAWN_FAILS__) throw "claude was not found on your PATH";
            return true;
          case "agent_send": case "agent_interrupt": case "agent_kill": return null;
          case "agent_decide": window.__DECIDED__.push({ ...args }); return true;
          // Stands in for `markdown::render_with`. Deliberately *not* a real
          // markdown renderer: what this page can honestly assert is that the
          // settled text goes through the command and the result lands in the
          // log, not that pulldown-cmark works — which `markdown.rs`'s own
          // tests cover, escaping included.
          case "render_agent_text": return `<p data-rendered="1">${args.text}</p>`;
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
/// `allowError` is for the one page that provokes a failure on purpose: a
/// `console.error` is otherwise a harness failure, and the pop-out's
/// cannot-start check needs the spawn to reject.
async function newPanePage(position, surface = "terminal", popout = "never", allowError = null) {
  const p = await newPage();
  p.on("pageerror", (e) => results.push(`FAIL pageerror (pane/${position}): ` + e.message));
  p.on("console", (m) => {
    if (m.type() !== "error") return;
    if (allowError && m.text().includes(allowError)) return;
    results.push(`FAIL console.error (pane/${position}): ` + m.text());
  });
  await p.addInitScript(paneStub, { base, position, surface, popout });
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

// --- the stack panel hands over to the pane -------------------------------
// Stack and send are adjacent because they are two halves of one gesture, and
// print is gone from the bar entirely — it is a Settings → Window row now.
check(
  "the stack toggle sits immediately left of send",
  await pane.evaluate(() =>
    document.getElementById("btn-send").previousElementSibling.id === "btn-stack"),
);
check(
  "and print has left the titlebar",
  await pane.evaluate(() => !document.getElementById("btn-print")),
);

// Sending from *inside* the panel is a substitution rather than a second panel:
// docked right the two occupy the same strip of window, so the queue closes and
// the conversation it produced opens where it was. The titlebar's send above is
// deliberately not this, which is why both are checked.
await seedPair(6);
await pane.evaluate(() => { closePane(); toggleStack(); });
await pane.waitForTimeout(200);
const sentBeforeHandoff = await sent();
await nudgeFlow(60_000);
await pane.locator("#btn-send-all").click();
await pane.waitForTimeout(400);
check("sending from the stack panel closes the panel", !(await pane.locator("#stack-panel.open").isVisible()));
check("and opens the pane", await pane.locator("#pty-pane.open").isVisible());
check("and still sends", (await sent()) === sentBeforeHandoff + 1, `${await sent()}`);

// Neither toggle is spent by the hand-off: the queue comes back with the pane
// up, and the pane goes away with the queue up. That independence is the whole
// reason this is a close rather than a mode.
await pane.evaluate(() => toggleStack());
await pane.waitForTimeout(200);
check(
  "the stack toggles back on with the pane still open",
  (await pane.locator("#stack-panel.open").isVisible()) &&
    (await pane.locator("#pty-pane.open").isVisible()),
);
await pane.evaluate(() => togglePane());
await pane.waitForTimeout(200);
check(
  "and the pane toggles off with the stack still open",
  !(await pane.locator("#pty-pane.open").isVisible()) &&
    (await pane.locator("#stack-panel.open").isVisible()),
);
// Back to what the next block expects: pane open, panel closed.
await pane.evaluate(() => closeStack());
await ensurePaneOpen();

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
//
// This page is `surface: "terminal"`, which matters: the registration branch
// below is *only* reachable there. The native surface is handed `--mcp-config`
// at spawn and cannot be unregistered, so a strip that told a native reader to
// run `claude mcp add` would be describing a problem they do not have — and the
// last check in this section is what holds that line.
const CMD = "claude mcp add dreamd --scope user -- /usr/bin/dreamd mcp";
const healthy = { armed: true, serving: true, registered: "yes", command: CMD };
check("a healthy socket says nothing", !(await pane.locator("#pty-pane.mcp-warn").count()));
const mcp = async (next) => {
  await pane.evaluate((m) => { window.__MCP__ = m; }, next);
  await pane.evaluate(() => refreshMcpStatus());
  await pane.waitForTimeout(150);
};
const strip = () => pane.locator("#pty-mcp").textContent();

await mcp({ ...healthy, registered: "no" });
check("an unregistered claude warns", (await pane.locator("#pty-pane.mcp-warn").count()) === 1);
check("and shows the command that fixes it", (await pane.locator("#pty-mcp code").textContent()) === CMD);
check(
  "with the scope flag that keeps it to one registration",
  (await strip()).includes("--scope user"),
);
check(
  "and a Copy button rather than one that runs it",
  (await pane.locator("#pty-mcp button").allTextContents()).join() === "Copy",
);
const copiedBefore = (await called("copy_to_clipboard")).length;
await pane.locator("#pty-mcp button").click();
await pane.waitForTimeout(150);
check(
  "Copy sends the command through the same path every other copy takes",
  (await called("copy_to_clipboard")).length === copiedBefore + 1
    && (await called("copy_to_clipboard")).at(-1).args.text === CMD,
);

// A `claude` that could not be run at all is a different sentence from one that
// ran and said no: reporting it as "not registered" sends the reader to fix a
// registration when the binary is what is missing.
await mcp({ ...healthy, registered: "unknown" });
check("an unreachable claude is hedged, not asserted", (await strip()).includes("could not ask"));
check("but the command is still offered", (await pane.locator("#pty-mcp code").textContent()) === CMD);

await mcp({ ...healthy, serving: false });
check("a secondary window warns too", (await pane.locator("#pty-pane.mcp-warn").count()) === 1);
check(
  "and offers no control, because none of them would help",
  (await pane.locator("#pty-mcp button").count()) === 0
    && (await pane.locator("#pty-mcp code").count()) === 0,
);
await mcp({ ...healthy, armed: false, serving: false });
check("and so does a window with no repo", (await pane.locator("#pty-pane.mcp-warn").count()) === 1);
check(
  "also with nothing to press",
  (await pane.locator("#pty-mcp button").count()) === 0,
);

// The regression this whole section exists for. The strip used to key on the
// client count, which the shim only moves on a *tool call* — so a correctly
// wired agent that had not needed dreamd yet was indistinguishable from an
// unregistered one, and the Register button it grew led to a Restart that
// repainted Register. A count is no longer an input at any value.
for (const clients of [0, 1, 99]) {
  await mcp({ ...healthy, clients });
  check(
    `a registered socket says nothing at clients=${clients}`,
    !(await pane.locator("#pty-pane.mcp-warn").count()),
  );
}

// Left as the boot state found it, because everything after this section
// assumes a quiet strip.
await mcp(healthy);

// --- the native agent surface ----------------------------------------------
// A page booted into `surface: "native"`, which is the default a reader gets.
// Everything above this belongs to the terminal, which is now the fallback.
//
// The seam being asserted is `agent-event` → DOM. Rust owns the half that turns
// Claude Code's stream-json into those events (`agent/wire.rs`, against
// committed fixtures) and this owns the half that turns them into a log. The
// events emitted below are the exact shapes `wire::AgentEvent` serializes to,
// which is what makes the two halves meet here rather than nowhere.
//
// As everywhere in this harness: what the page *knows*, not what it paints.
const nat = await newPanePage("bottom", "native");
await nat.keyboard.press("Control+t");
await nat.waitForFunction(() => typeof agent !== "undefined" && agent.running, null, { timeout: 15000 })
  .catch(() => {});

check("the native body is the one shown", await nat.locator("#pty-pane.native #agent-body").isVisible());
check("and xterm was never loaded for it", await nat.evaluate(() => !window.Terminal));

// A turn: streaming deltas, then the settled block that replaces them.
await nat.evaluate(() => {
  window.__EMIT__("agent-event", { kind: "ready", sessionId: "s1", model: "claude-haiku-4-5-20251001" });
  window.__EMIT__("agent-event", { kind: "status", status: "requesting" });
  window.__EMIT__("agent-event", { kind: "textDelta", index: 0, text: "The whitespace tier " });
  window.__EMIT__("agent-event", { kind: "textDelta", index: 0, text: "scans the whole source." });
});
check(
  "deltas stream in as plain text",
  (await nat.locator("#agent-log .agent-said.streaming").innerText()).includes("scans the whole source"),
);
check(
  "and the model chip lights from the session, not from a click",
  await nat.evaluate(() => !!document.querySelector('#pty-models .pty-model[data-model="haiku"].sel'),
));

await nat.evaluate(() => {
  window.__EMIT__("agent-event", { kind: "toolStart", id: "t1", name: "Read", target: "src/markdown.rs" });
  window.__EMIT__("agent-event", { kind: "toolEnd", id: "t1", ok: true });
  window.__EMIT__("agent-event", { kind: "toolStart", id: "t2", name: "Bash", target: "cargo build" });
  window.__EMIT__("agent-event", { kind: "toolEnd", id: "t2", ok: false });
});
check("a tool call ticks when it returns", await nat.evaluate(() =>
  [...document.querySelectorAll(".agent-tool")].some(
    (r) => r.querySelector(".t-name").textContent === "Read" && r.querySelector(".t-mark").textContent === "✓")));
check("and a refused one is crossed", await nat.evaluate(() =>
  [...document.querySelectorAll(".agent-tool.failed")].some(
    (r) => r.querySelector(".t-mark").textContent === "✗")));

// The settled block goes through `render_agent_text`.
await nat.evaluate(() =>
  window.__EMIT__("agent-event", { kind: "text", index: 0, text: "The whitespace tier scans the whole source." }));
await nat.waitForFunction(() => !!document.querySelector("#agent-log [data-rendered]"), null, { timeout: 5000 })
  .catch(() => {});
check(
  "the settled block is replaced by the rendered markdown",
  await nat.evaluate(() => !!document.querySelector("#agent-log [data-rendered]")),
);
check(
  "and it stops being a streaming node",
  await nat.evaluate(() => !document.querySelector("#agent-log .agent-said.streaming")),
);

// A permission card, and the three answers it can carry.
await nat.evaluate(() =>
  window.__EMIT__("agent-ask", { id: "t3", tool: "Bash", input: { command: "rm -rf build" } }));
check("a card names the tool", (await nat.locator(".agent-card .c-what").innerText()).includes("Bash"));
check(
  "and shows the call rather than its JSON",
  (await nat.locator(".agent-card .c-detail").innerText()).trim() === "rm -rf build",
);
await nat.locator(".agent-card button", { hasText: "Always allow" }).click();
check("“always” sends allow and always", await nat.evaluate(() => {
  const d = window.__DECIDED__.at(-1);
  return d && d.id === "t3" && d.allow === true && d.always === true;
}));
check(
  "and the answered card settles rather than vanishing",
  await nat.locator(".agent-card.settled").count() === 1,
);

// The turn ends, and everything scoped to it goes with it — otherwise the next
// answer's block 0 would overwrite this one's.
await nat.evaluate(() =>
  window.__EMIT__("agent-event", { kind: "turn", ok: true, interrupted: false, costUsd: 0.01, durationMs: 10, denials: 1 }));
check("a turn's block map is cleared with it", await nat.evaluate(() => agent.blocks.size === 0));
check("and a denial is reported to the reader", await nat.evaluate(() =>
  [...document.querySelectorAll(".agent-note")].some((n) => n.textContent.includes("not allowed"))));

// The composer. Enter sends, Shift+Enter does not.
await nat.locator("#agent-input").fill("why is locate slow here?");
await nat.keyboard.press("Shift+Enter");
check("Shift+Enter does not send", await nat.evaluate(() =>
  window.__CALLS__.filter((c) => c.cmd === "agent_send").length === 0));
await nat.locator("#agent-input").fill("why is locate slow here?");
await nat.keyboard.press("Enter");
check("Enter sends the composer", await nat.evaluate(() => {
  const sent = window.__CALLS__.filter((c) => c.cmd === "agent_send");
  return sent.length === 1 && sent[0].args.text === "why is locate slow here?";
}));
check("and the composer is cleared", await nat.evaluate(() => document.getElementById("agent-input").value === ""));
check("and the reader's turn is in the log", await nat.evaluate(() =>
  [...document.querySelectorAll(".agent-turn.you .agent-said")].some((n) => n.textContent.includes("locate slow"))));

// Escape interrupts a running turn instead of closing the pane — the thing the
// terminal surface could not do, because xterm claimed the key.
await nat.evaluate(() => window.__EMIT__("agent-event", { kind: "status", status: "requesting" }));
await nat.locator("#agent-input").focus();
await nat.keyboard.press("Escape");
check("Escape interrupts a running turn", await nat.evaluate(() =>
  window.__CALLS__.some((c) => c.cmd === "agent_interrupt")));
check("and leaves the pane open", await nat.locator("#pty-pane.open").isVisible());

// A model chip is a turn natively, not a separate command.
await nat.evaluate(() => window.__EMIT__("agent-event", { kind: "turn", ok: true, interrupted: true, costUsd: 0, durationMs: 1, denials: 0 }));
await nat.locator('#pty-models .pty-model[data-model="opus"]').click();
check("a model chip sends a slash command as a turn", await nat.evaluate(() =>
  window.__CALLS__.some((c) => c.cmd === "agent_send" && c.args.text === "/model opus")));
check("and never reaches the pty command", await nat.evaluate(() =>
  !window.__CALLS__.some((c) => c.cmd === "pty_model")));

// ...and it lights *now*, which is the half a turn cannot deliver. `system/init`
// is emitted at the top of a turn, before the CLI has read the line that turn
// carries, so the `ready` following this click still reports the model the
// session was already on. Painting from `ready` alone therefore repainted the
// chip the reader had just left and the press only appeared one turn later —
// every chip took two clicks. The press paints; the wire reconciles.
const lit = () => nat.evaluate(() =>
  document.querySelector("#pty-models .pty-model.sel")?.dataset.model ?? null);
check("a model chip lights on the press", (await lit()) === "opus", await lit());
await nat.evaluate(() => window.__EMIT__("agent-event",
  { kind: "ready", sessionId: "s1", model: "claude-haiku-4-5-20251001" }));
check("a stale init does not steal the chip back", (await lit()) === "opus", await lit());
await nat.evaluate(() => window.__EMIT__("agent-event",
  { kind: "ready", sessionId: "s1", model: "claude-opus-4-5-20260101" }));
check("and the confirming init keeps it lit", (await lit()) === "opus", await lit());

// Bounded, so a `/model` the CLI never honoured costs one more turn rather than
// the session: two inits that disagree and the wire wins regardless.
await nat.locator('#pty-models .pty-model[data-model="sonnet"]').click();
check("an unconfirmed press still paints", (await lit()) === "sonnet", await lit());
for (let i = 0; i < 2; i++) {
  await nat.evaluate(() => window.__EMIT__("agent-event",
    { kind: "ready", sessionId: "s1", model: "claude-opus-4-5-20260101" }));
}
check("but the wire wins after two disagreeing inits", (await lit()) === "opus", await lit());

await nat.close();

// --- the pop-out (`agent.popout`) -------------------------------------------
// Three pages, because `popout` is read once on the pane's first open — the
// same reason `agent.position` gets one per value below.
//
// What is asserted is *where the conversation is* and *who has the keyboard*:
// the card holds one moved `#agent-body`, not a second copy of it, and the
// composer is absent until asked for. Whether the card is pretty, centred and
// shadowed is CSS in a browser that is not WebKitGTK, and stays a hand-check.
const pop = await newPanePage("right", "native", "always");
await pop.keyboard.press("Control+t");
await pop.waitForFunction(() => typeof agent !== "undefined" && agent.running, null, { timeout: 15000 })
  .catch(() => {});
await pop.waitForTimeout(300);

check("`always` raises the card instead of the dock", await pop.locator("#agent-popout.open").isVisible());
check("and the dock stays shut", !(await pop.locator("#pty-pane.open").isVisible()));
// The move, not a copy: one `#agent-body` in the document, and it is the card's.
check("the conversation moved rather than being duplicated", await pop.evaluate(() =>
  document.querySelectorAll("#agent-body").length === 1 &&
  document.getElementById("agent-body").parentElement.id === "agent-card"));
check("it opens read-only", !(await pop.locator("#agent-composer").isVisible()));
// The compositing promotion, pinned because it reads as decoration and is not:
// an overflow box's scrollbar does not take part in z-index, and on GTK it is
// drawn into a composited scrolling layer that paints over plain content
// whatever that content's z-index says. `#tree`'s scrollbar drew over this
// card. A layer of its own is what puts the two in the same comparison.
// Chromium computes the same property; whether it cures the paint order is a
// WebKitGTK question and stays a hand-check.
check("the card is promoted to its own layer", await pop.evaluate(() => {
  const t = getComputedStyle(document.getElementById("agent-popout")).transform;
  return !!t && t !== "none";
}), await pop.evaluate(() => getComputedStyle(document.getElementById("agent-popout")).transform));
check("and takes the keyboard, so `i` has something to mean", await pop.evaluate(() =>
  document.activeElement === document.getElementById("agent-card")));

await pop.keyboard.press("i");
await pop.waitForTimeout(150);
check("`i` reveals the composer", await pop.locator("#agent-popout.editing #agent-composer").isVisible());
check("and puts the caret in it", await pop.evaluate(() =>
  document.activeElement === document.getElementById("agent-input")));

// Escape's two jobs, in order. The first gives the composer back; only the
// second puts the card away — which is why one press must not do both.
await pop.keyboard.press("Escape");
await pop.waitForTimeout(150);
check("Escape gives the composer back first", await pop.evaluate(() =>
  !popout.editing && document.getElementById("agent-popout").classList.contains("open")));
await pop.keyboard.press("Escape");
await pop.waitForTimeout(200);
check("and a second one lowers the card", !(await pop.locator("#agent-popout.open").isVisible()));
check("returning the conversation to the dock intact", await pop.evaluate(() =>
  document.getElementById("agent-body").parentElement.id === "pty-pane" &&
  document.getElementById("pty-mcp").nextElementSibling.id === "pty-confirm"));

// The other way in. A press on a control is an answer, not a request to type —
// otherwise answering a permission card in the card would grow a textarea under
// the cursor every time.
await pop.keyboard.press("Control+t");
await pop.waitForTimeout(250);
await pop.evaluate(() =>
  window.__EMIT__("agent-ask", { id: "t9", tool: "Bash", input: { command: "ls" } }));
await pop.locator(".agent-card button", { hasText: "Deny" }).click();
await pop.waitForTimeout(150);
check("answering a permission card is not a request to type", await pop.evaluate(() => !popout.editing));
await pop.locator("#agent-log").click();
await pop.waitForTimeout(150);
check("but clicking the log itself is", await pop.locator("#agent-popout.editing").isVisible());

// The MCP strip travels with the body. In `always` the dock never opens, so a
// warning left behind in it is a warning nobody is ever shown.
//
// The losing-the-socket case rather than the registration one, deliberately:
// this page is `native`, where the registration branch is unreachable by
// construction, and a second window owning the socket is the failure that can
// still befall it.
await pop.evaluate(() => {
  window.__MCP__ = { armed: true, serving: false, registered: "yes", command: "" };
  return refreshMcpStatus();
});
await pop.waitForTimeout(200);
check("the MCP warning is visible in the card", await pop.locator("#agent-popout #pty-mcp").isVisible());
// The native surface's whole point: it is handed `--mcp-config` at spawn, so an
// unregistered `claude` is not a thing it can be. A strip that told this reader
// to run `claude mcp add` would be describing someone else's problem.
await pop.evaluate(() => {
  window.__MCP__ = { armed: true, serving: true, registered: "no", command: "claude mcp add …" };
  return refreshMcpStatus();
});
await pop.waitForTimeout(200);
check(
  "but an unregistered claude is not the native surface's problem",
  !(await pop.locator("#agent-popout.mcp-warn").count()),
);
await pop.close();

// `send` is the middle answer and the reason the setting is not a bool: the
// toggle still docks, and only the stack hand-off raises the card.
const popSend = await newPanePage("right", "native", "send");
await popSend.keyboard.press("Control+t");
await popSend.waitForTimeout(400);
check("`send` leaves the pane's own toggle on the dock", await popSend.locator("#pty-pane.open").isVisible());
check("and raises nothing", !(await popSend.locator("#agent-popout.open").isVisible()));
// D10: an empty stack still opens the agent, which is what makes this a check of
// the *route* rather than of the queue.
await popSend.keyboard.press("Control+Enter");
await popSend.waitForTimeout(500);
check("but a stack send raises the card", await popSend.locator("#agent-popout.open").isVisible());
check("and the dock it was in gives the conversation up", await popSend.evaluate(() =>
  document.getElementById("agent-body").parentElement.id === "agent-card" &&
  !document.getElementById("pty-pane").classList.contains("open")));
await popSend.close();

// A session that will not start. The dock puts this in `#pty-status`; the card
// has no header to put it in, so the hint line carries it — and the check that
// matters is that it stops saying "starting", which would report the failure as
// patience and leave the reader waiting on a process that does not exist.
const popDead = await newPanePage("right", "native", "always", "not found on your PATH");
await popDead.evaluate(() => { window.__SPAWN_FAILS__ = true; });
await popDead.keyboard.press("Control+t");
await popDead.waitForTimeout(500);
check("a card that cannot start says why", await popDead.evaluate(() => {
  const t = document.getElementById("agent-hint").textContent;
  return t.includes("not found") && !t.includes("starting");
}), await popDead.evaluate(() => document.getElementById("agent-hint").textContent));
await popDead.close();

// The fallback surface never pops out, whatever the config says: xterm needs a
// box the fit addon manages, and this card grows a composer on demand.
const popTerm = await newPanePage("right", "terminal", "always");
await popTerm.keyboard.press("Control+t");
await popTerm.waitForTimeout(600);
check("the terminal surface ignores the pop-out and docks", await popTerm.evaluate(() =>
  document.getElementById("pty-pane").classList.contains("open") &&
  !document.getElementById("agent-popout").classList.contains("open")));
await popTerm.close();

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
  const m = document.getElementById("main-wrap").getBoundingClientRect();
  return { px: Math.round(p.x), pw: Math.round(p.width), ph: Math.round(p.height),
           dr: Math.round(d.right), mh: Math.round(m.height) };
});
// Beside the document rather than under it, and spanning both rows of
// `#main-wrap` — the shape a bottom dock cannot produce, so this fails if the
// grid never applied.
//
// Against `#main-wrap` rather than against the document, which is what it used
// to compare with. Under `ui.titlebar_fade` the scroller is pulled 38px up under
// the bar and is *taller* than the box it lives in, so "as tall as the document"
// stopped being the same claim as "full height" on exactly one platform.
check(
  "it sits beside the document, not under it",
  geom.px >= geom.dr && geom.pw > 200 && geom.ph >= geom.mh - 1,
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

// The drag handle picks its axis at press time, not at wiring time — which is
// exactly what the flip above would break if it did not. So this runs *after*
// a round trip through the other dock, and asserts on the key it wrote as much
// as on the size: a right dock that persisted `pane_height` would look correct
// until the reader switched back.
const paneWidthPx = () =>
  right.evaluate(() => Math.round(document.getElementById("pty-pane").getBoundingClientRect().width));
const paneRight = await right.evaluate(() =>
  Math.round(document.getElementById("pty-pane").getBoundingClientRect().right));
await dragHandleTo(right, "pane-resize", { x: paneRight - 460 });
check("dragging the right-docked pane sets its width", (await paneWidthPx()) === 460, `${await paneWidthPx()}`);
await right.waitForTimeout(600);
const rightPatch = await right.evaluate(() =>
  window.__CALLS__.filter((c) => c.cmd === "set_config" && c.args.patch.ui).map((c) => c.args.patch.ui));
check("and writes the pane's width, not its height",
  rightPatch.length >= 1 &&
    rightPatch[rightPatch.length - 1].pane_width === 460 &&
    rightPatch.every((p) => p.pane_height == null),
  JSON.stringify(rightPatch));

// --- the header folds instead of being cut off ----------------------------
// `#pty-pane` is `overflow: hidden`, so a header row wider than the dock loses
// its right end silently — and the right end is ✕. Dragged to the 240px minimum
// the row is far wider than the dock, so this is the size that proves it wraps.
// Measured against the head's *content* box, because the padding is not room.
await dragHandleTo(right, "pane-resize", { x: paneRight - 240 });
await right.waitForTimeout(400);
const folded = await right.evaluate(() => {
  const head = document.getElementById("pty-head");
  const box = head.getBoundingClientRect();
  const cs = getComputedStyle(head);
  const limit = box.right - parseFloat(cs.paddingRight);
  const rows = new Set();
  let worst = 0;
  for (const el of head.children) {
    const r = el.getBoundingClientRect();
    if (!r.width && !r.height) continue;      // an empty #pty-status
    worst = Math.max(worst, r.right - limit);
    rows.add(Math.round(r.top));
  }
  return { worst: Math.round(worst), rows: rows.size, h: Math.round(box.height) };
});
check(
  "at the minimum dock width nothing in the pane header overhangs it",
  folded.worst <= 1,
  JSON.stringify(folded),
);
check(
  "because the header folded onto more than one row",
  folded.rows > 1,
  JSON.stringify(folded),
);
// Right-justified, not ragged: every folded row's last item ends at the same
// edge. The title's row is exempt — it grows to fill, which is what keeps the
// unwrapped header looking exactly as it always did.
const ragged = await right.evaluate(() => {
  const head = document.getElementById("pty-head");
  const limit = head.getBoundingClientRect().right - parseFloat(getComputedStyle(head).paddingRight);
  const items = [...head.children]
    .map((el) => ({ el, r: el.getBoundingClientRect() }))
    .filter((i) => i.r.width || i.r.height);
  // Grouped by vertical *overlap*, not by `top`. `align-items: center` centres
  // a 7px dot against a 17px title, so two items on one flex line do not share
  // a top — grouping by that number splits the row the dot is on and reports it
  // as a line 212px short of the edge.
  const lines = [];
  for (const { el, r } of items) {
    const line = lines.find((l) => r.top < l.bottom && r.bottom > l.top);
    if (line) {
      line.top = Math.min(line.top, r.top);
      line.bottom = Math.max(line.bottom, r.bottom);
      line.right = Math.max(line.right, r.right);
      line.els.push(el);
    } else lines.push({ top: r.top, bottom: r.bottom, right: r.right, els: [el] });
  }
  const title = document.getElementById("pty-title");
  return lines.filter((l) => !l.els.includes(title)).map((l) => Math.round(limit - l.right));
});
check(
  "and every folded row is flush with the right edge",
  ragged.length > 0 && ragged.every((gap) => gap <= 1),
  JSON.stringify(ragged),
);
await dragHandleTo(right, "pane-resize", { x: paneRight - 460 });
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
    toggle_mode: "Ctrl+Shift+D",
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
    config: {
      theme: "dreamd", mode: "system", extra_ignores: [], keymap: KEYMAP,
      ui: { tree_width: 320, stack_width: 360, pane_width: 500, pane_height: 300 },
    },
    theme: "dreamd", scheme: "dark", system: "dark", themes: [], syntax_themes: [],
    config_path: "/tmp/xdg/dreamd/config.toml", themes_dir: "/tmp/xdg/dreamd/themes",
    local_overrides: [],
  });

  const body = "<h1 id='a'>One</h1>" + "<p>filler</p>".repeat(80) +
    "<h2 id='b'>Two</h2>" + "<p>filler</p>".repeat(80) +
    "<h2 id='c'>Three</h2>" + "<p>filler</p>".repeat(80);

  window.__TAURI__ = {
    core: {
      // Tauri's own, and the only URL a local image ever gets: the CSP
      // admits `asset:` and not `file:`. The test image resolves to a
      // data URI so Chromium can actually decode one; every other path
      // comes back in the real shape, and `__ASSET__` records what the
      // containment guard handed over.
      convertFileSrc(path) {
        (window.__ASSET__ ||= []).push(path);
        return path.endsWith("pic.png")
          ? window.__PNG__
          : "asset://localhost/" + encodeURIComponent(path);
      },
      async invoke(cmd, args) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info":
            return { root: state.root, name: state.root.split("/").pop(), display: state.root };
          case "get_keymap": return KEYMAP;
          // Persisted sizes, deliberately none of them the default: a boot
          // that never asked would still be sitting at 260/280/240 and look
          // correct.
          case "get_ui": return { tree_width: 320, stack_width: 360, pane_width: 500, pane_height: 300 };
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
          case "agent_prefs": return { position: "bottom", permission_mode: "accept-edits", surface: "native" };
          case "agent_spawn": return true;
          case "agent_send": case "agent_interrupt": case "agent_kill": return null;
          case "pty_spawn": return true;
          case "pty_write": case "pty_resize": case "pty_kill": case "pty_model": return null;
          // This page opens the pane as a side effect of pressing every
          // binding, so it needs an answer here too: `refreshMcpStatus` runs on
          // every open, and a bare `null` would be one more pageerror to chase.
          case "mcp_status":
            return { armed: true, serving: true, registered: "yes", command: "" };
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

// --- 4a0. the sidebar owns the top-left corner ---
// `#workspace` is a 2x2 grid with the sidebar spanning both rows, so the tree
// reaches the top of the window and the bar spans only the document beside it.
// Geometry rather than class names: the whole point of the grid is where the
// boxes land, and a `grid-area` typo leaves every class exactly where it was.
const chromeMac = await chrome.evaluate(() => document.body.classList.contains("mac"));
const topGeom = () => chrome.evaluate(() => {
  const s = document.getElementById("sidebar").getBoundingClientRect();
  const t = document.getElementById("titlebar").getBoundingClientRect();
  return {
    st: Math.round(s.top), sh: Math.round(s.height), sr: Math.round(s.right),
    tt: Math.round(t.top), tl: Math.round(t.left),
    lights: getComputedStyle(document.getElementById("sidebar-lights")).display,
    pad: getComputedStyle(document.getElementById("titlebar")).paddingLeft,
    wh: Math.round(window.innerHeight),
  };
});
let g = await topGeom();
check("the open sidebar reaches the top of the window", g.st === 0, JSON.stringify(g));
check("and runs its full height", g.sh === g.wh, JSON.stringify(g));
check("the bar starts where the tree ends", g.tl === g.sr && g.tt === 0, JSON.stringify(g));
// The lights' room is the sidebar's while the tree is open, so the bar owes them
// nothing — 10px is `#titlebar`'s own padding, not a gutter.
check(
  "the lights' strip is the sidebar's, on macOS only",
  g.lights === (chromeMac ? "block" : "none"),
  JSON.stringify(g),
);
check("and the bar keeps its ordinary padding", g.pad === "10px", JSON.stringify(g));

await chrome.keyboard.press("Control+b");
await chrome.waitForTimeout(120);
g = await topGeom();
check("collapsing hands the corner back to the bar", g.tl === 0, JSON.stringify(g));
check("the sidebar's strip goes with it", g.lights === "none", JSON.stringify(g));
// The other half of the same handover, and the one a reader notices: with no
// tree in the corner the bar has to clear the traffic lights itself.
check(
  "and the bar takes the 78px gutter on macOS",
  g.pad === (chromeMac ? "78px" : "10px"),
  JSON.stringify(g),
);
await chrome.keyboard.press("Control+b");
await chrome.waitForTimeout(120);
check("expanding gives it back", (await topGeom()).pad === "10px");

// Dragged well past the maximum: the clamp is the frontend's, so the config
// file never has to reject what the handle sent.
//
// Takes the handle by id, because the tree is not the only thing with one:
// `grab` picks a point on it that is *on* the element in both axes, and the
// caller says where to release. All three handles share `wireDrag`, so what
// each of these checks is really pinning is the one thing that differs — which
// fixed edge the pointer is measured from.
// A declaration rather than a `const`: the right-dock page above drags too, and
// it runs several hundred lines before this point.
async function dragHandleTo(page, id, to, steps = 6) {
  const from = await page.evaluate((h) => {
    const r = document.getElementById(h).getBoundingClientRect();
    return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
  }, id);
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(to.x ?? from.x, to.y ?? from.y, { steps });
  await page.mouse.up();
  await page.waitForTimeout(80);
}
const dragTo = (x, steps = 6) => dragHandleTo(chrome, "tree-resize", { x }, steps);

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

// --- 4a-bis. the stack and pane drags ---
// The same mechanism on two more edges. Each check pins the one thing that
// differs between the three handles and the one thing a shared `wireDrag`
// could get backwards: which fixed edge the pointer is measured from, and so
// which way the panel grows under it.
const stackWidthPx = () =>
  chrome.evaluate(() => Math.round(document.getElementById("stack-panel").getBoundingClientRect().width));
const paneHeightPx = () =>
  chrome.evaluate(() => Math.round(document.getElementById("pty-pane").getBoundingClientRect().height));
const uiPatches = (key) =>
  chrome.evaluate((k) => window.__STATE__.patches.map((p) => p.ui && p.ui[k]).filter((n) => n != null), key);

await chrome.keyboard.press("Control+o");
await chrome.waitForTimeout(120);
check("the persisted stack width is applied on boot", (await stackWidthPx()) === 360, `${await stackWidthPx()}`);

await dragHandleTo(chrome, "stack-resize", { x: 980 });
check("the stack panel grows leftwards from its own right edge",
  (await stackWidthPx()) === 300, `${await stackWidthPx()}`);
// One move, so the width the drag last applied is not whatever the pointer
// happened to cross on the way — the same reason the tree's collapse drag
// above uses a single step.
await dragHandleTo(chrome, "stack-resize", { x: 1260 }, 1);
check("and stops at its minimum rather than collapsing",
  (await stackWidthPx()) === 200 && (await chrome.locator("#stack-panel.open").isVisible()),
  `${await stackWidthPx()}`);
await dragHandleTo(chrome, "stack-resize", { x: 300 });
check("and at its maximum going the other way", (await stackWidthPx()) === 720, `${await stackWidthPx()}`);
await dragHandleTo(chrome, "stack-resize", { x: 980 });

await chrome.keyboard.press("Control+t");
await chrome.waitForTimeout(500);
check("the persisted pane height is applied on its first open",
  (await paneHeightPx()) === 300, `${await paneHeightPx()}`);
// The stack panel stops at the pane rather than covering it, and it is
// `--pane-height` that tells it where the pane now ends — the one thing the
// indirection through `#main-wrap`'s `--pane-h` exists to keep true.
check("and the stack panel comes up short of it",
  await chrome.evaluate(() => {
    const s = document.getElementById("stack-panel").getBoundingClientRect();
    const p = document.getElementById("pty-pane").getBoundingClientRect();
    return Math.abs(s.bottom - p.top) <= 1;
  }));

const paneBottom = await chrome.evaluate(() =>
  Math.round(document.getElementById("pty-pane").getBoundingClientRect().bottom));
await dragHandleTo(chrome, "pane-resize", { y: paneBottom - 420 });
check("the pane grows upwards from the bottom edge it is docked against",
  (await paneHeightPx()) === 420, `${await paneHeightPx()}`);
check("and the stack panel follows it up",
  await chrome.evaluate(() => {
    const s = document.getElementById("stack-panel").getBoundingClientRect();
    const p = document.getElementById("pty-pane").getBoundingClientRect();
    return Math.abs(s.bottom - p.top) <= 1;
  }));
await dragHandleTo(chrome, "pane-resize", { y: paneBottom - 20 }, 1);
check("and stops at its minimum rather than closing",
  (await paneHeightPx()) === 120 && (await chrome.locator("#pty-pane.open").isVisible()),
  `${await paneHeightPx()}`);

// Debounced like the tree's, and into the same accumulating patch — so what
// reaches `set_config` is one write per drag, never a raw pointer position.
await chrome.waitForTimeout(600);
const stackPatched = await uiPatches("stack_width");
const panePatched = await uiPatches("pane_height");
check("the stack drag persists its width",
  stackPatched.length >= 1 && stackPatched[stackPatched.length - 1] === 300,
  JSON.stringify(stackPatched));
check("and never sends one outside the clamp",
  stackPatched.every((n) => n >= 200 && n <= 720), JSON.stringify(stackPatched));
check("the pane drag persists its height",
  panePatched.length >= 1 && panePatched[panePatched.length - 1] === 120,
  JSON.stringify(panePatched));
check("and never sends one outside the clamp",
  panePatched.every((n) => n >= 120 && n <= 1200), JSON.stringify(panePatched));
// A bottom-docked pane is a height. Writing the width from this drag would be
// invisible until the reader switched docks and found the pane the wrong size.
check("a bottom dock never writes the pane's width",
  (await uiPatches("pane_width")).length === 0);

await chrome.keyboard.press("Control+t");
await chrome.keyboard.press("Control+o");
await chrome.waitForTimeout(150);

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

// --- one highlight per passage, and resizing the one that is there ---
// A page with a real (if tiny) store behind the IPC stub, because both halves
// of this feature are about what the *store* ends up holding after a gesture:
// refusing an overlapping selection is only meaningful if no `add_highlight`
// went out, and a resize is only meaningful if the same id came back with a
// different quote.
//
// Overlap is decided against the painted DOM (`overlappingIds`), so a stub that
// merely counted calls would assert nothing. The document below is one
// paragraph of five words for that reason: every case — a quote inside the
// mark, one straddling its start, one clear of it — is a substring of it.
const hl = await newPage();
hl.on("pageerror", (e) => results.push("FAIL pageerror (highlights): " + e.message));
hl.on("console", (m) => {
  if (m.type() === "error") results.push("FAIL console.error (highlights): " + m.text());
});
await hl.addInitScript(({ base }) => {
  const tree = {
    name: "repo", is_dir: true, path: "/repo", rel: "",
    children: [{ name: "doc.md", is_dir: false, path: "/repo/doc.md", rel: "doc.md", children: [] }],
  };
  // Deliberately flat: no inline markup, so every quote below fits in one text
  // node and a failure is about the guard rather than about `placeAcrossNodes`.
  const store = { marks: [], stack: [], seq: 0, resizes: 0 };
  window.__STORE__ = store;
  const find = (id) => store.marks.find((m) => m.id === id);
  window.__TAURI__ = {
    core: {
      // Tauri's own, and the only URL a local image ever gets: the CSP
      // admits `asset:` and not `file:`. The test image resolves to a
      // data URI so Chromium can actually decode one; every other path
      // comes back in the real shape, and `__ASSET__` records what the
      // containment guard handed over.
      convertFileSrc(path) {
        (window.__ASSET__ ||= []).push(path);
        return path.endsWith("pic.png")
          ? window.__PNG__
          : "asset://localhost/" + encodeURIComponent(path);
      },
      async invoke(cmd, args) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return {
            palette: "Ctrl+F", palette_prev: "Ctrl+P", palette_next: "Ctrl+N",
            highlight: "Ctrl+H", send_stack: "Ctrl+Enter", toggle_stack: "Ctrl+O",
            toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B", toggle_view: "Ctrl+M",
            toggle_mode: "Ctrl+Shift+D",
            jump_top: "Home", jump_bottom: "End", set_mark: "m", jump_mark: "'",
            jump_back: "Ctrl+[", jump_forward: "Ctrl+]",
            find: "/", find_next: "n", find_prev: "Shift+N",
            next_file: "]", prev_file: "[",
            copy_stack: "Ctrl+C", settings: "Ctrl+,",
            save_annotation: "Ctrl+Y",
            // Off, so a bare `h` cannot stand in for the configured key and the
            // assertions below are about the binding they press.
            quick_highlight: false,
          };
          case "get_theme": return { css: base, mode: "system", scheme: "dark", syntax_theme: null };
          case "initial_file": return "/repo/doc.md";
          case "list_markdown_files": return tree;
          case "render_markdown": return "<p id=\"p\">alpha beta gamma delta epsilon</p>";
          case "add_highlight": {
            const id = "h" + ++store.seq;
            store.marks.push({
              id, file_path: args.filePath, quote: args.quote,
              prefix: args.prefix, suffix: args.suffix,
              line_start: 1, line_end: 1, state: "active", annotation: null,
            });
            return id;
          }
          case "resize_highlight": {
            const m = find(args.id);
            if (!m) return false;
            m.quote = args.quote; m.prefix = args.prefix; m.suffix = args.suffix;
            store.resizes++;
            return true;
          }
          case "set_annotation": {
            const m = find(args.id);
            if (!m) return false;
            m.annotation = args.text;
            if (!store.stack.includes(args.id)) store.stack.push(args.id);
            return true;
          }
          case "remove_highlight":
            store.marks = store.marks.filter((m) => m.id !== args.id);
            store.stack = store.stack.filter((x) => x !== args.id);
            return null;
          case "get_highlight": return find(args.id) || null;
          case "get_highlights": case "reanchor":
            return store.marks.filter((m) => m.file_path === args.path);
          case "get_stack":
            return store.stack
              .map(find)
              .filter((m) => m && m.annotation)
              .map((h) => ({ highlight: h, annotation: h.annotation }));
          default: return null;
        }
      },
    },
    event: { async listen() { return () => {}; } },
  };
}, { base });
await hl.goto(pathToFileURL(join(UI, "index.html")).href);
await hl.waitForFunction(() => document.getElementById("content").textContent.includes("epsilon"));

// Select a stretch of rendered text by its content, across text nodes. Across,
// because the moment one mark is painted the paragraph is three nodes and every
// overlapping case straddles a boundary — the same reason `placeAcrossNodes`
// exists.
const selectText = (needle) => hl.evaluate((needle) => {
  const nodes = [], starts = [];
  let total = 0, text = "";
  const w = document.createTreeWalker(document.getElementById("content"), NodeFilter.SHOW_TEXT);
  for (let n; (n = w.nextNode()); ) {
    nodes.push(n); starts.push(total); total += n.nodeValue.length; text += n.nodeValue;
  }
  const at = text.indexOf(needle);
  if (at < 0) return false;
  const point = (off) => {
    let i = 0;
    while (i + 1 < starts.length && starts[i + 1] <= off) i++;
    return [nodes[i], off - starts[i]];
  };
  const [sn, so] = point(at);
  const [en, eo] = point(at + needle.length);
  const r = document.createRange();
  r.setStart(sn, so); r.setEnd(en, eo);
  const s = window.getSelection();
  s.removeAllRanges(); s.addRange(r);
  return true;
}, needle);
const marksState = () => hl.evaluate(() => ({
  count: window.__STORE__.marks.length,
  quotes: window.__STORE__.marks.map((m) => m.quote),
  resizes: window.__STORE__.resizes,
  stack: window.__STORE__.stack.length,
}));

check("the fixture paragraph is selectable", await selectText("beta gamma"));
await hl.keyboard.press("Control+H");
await hl.waitForSelector("#annot-overlay.open");
await hl.locator("#annot-text").fill("why this?");
await hl.locator("#annot-save").click();
await hl.waitForTimeout(200);
check("a first highlight is created and painted",
  (await marksState()).count === 1 && (await hl.locator("mark.hl").count()) > 0);

// The rule. `alpha beta` starts outside the mark and ends inside it.
await selectText("alpha beta");
await hl.keyboard.press("Control+H");
await hl.waitForTimeout(200);
let state = await marksState();
check("an overlapping selection mints no second mark", state.count === 1, `${state.count}`);
check("and opens the existing mark for editing instead",
  await hl.locator("#annot-overlay.open").isVisible() &&
  (await hl.locator("#annot-title").textContent()) === "Edit annotation");
check("with the existing annotation in the box",
  (await hl.locator("#annot-text").inputValue()) === "why this?");
check("and the resize button is offered", await hl.locator("#annot-resize").isVisible());
await hl.keyboard.press("Escape");
await hl.waitForTimeout(150);

// The other half of the same guard: text merely *next to* a mark is still new
// text. Without this the check above would pass on a rule that refused
// everything.
await selectText("delta epsilon");
await hl.keyboard.press("Control+H");
await hl.waitForTimeout(200);
state = await marksState();
check("a selection clear of every mark still highlights", state.count === 2, `${state.count}`);
check("and does so as a new mark, not an edit",
  (await hl.locator("#annot-title").textContent()) === "Add annotation");
await hl.keyboard.press("Escape"); // cancels a create, so the mark goes with it
await hl.waitForTimeout(200);
check("cancelling that create removes it again", (await marksState()).count === 1);

// --- resizing ---
await hl.locator("mark.hl").first().click();
await hl.waitForSelector("#annot-overlay.open");
await hl.locator("#annot-resize").click();
await hl.waitForTimeout(150);
check("Resize closes the modal and arms the mode",
  !(await hl.locator("#annot-overlay.open").isVisible()) &&
  await hl.evaluate(() => document.body.classList.contains("resizing")));
check("the hint bar is up", await hl.locator("#resize-hint").isVisible());
check("and the mark says which one is being redrawn",
  (await hl.locator("mark.hl.resizing").count()) > 0);

// Escape leaves the mark exactly as it was.
await hl.keyboard.press("Escape");
await hl.waitForTimeout(150);
state = await marksState();
check("Escape leaves resize mode",
  !(await hl.evaluate(() => document.body.classList.contains("resizing"))));
check("and changes nothing", state.resizes === 0 && state.quotes[0] === "beta gamma");

// Shrink it, from the stack panel's button — the path a pair on the stack takes.
await hl.locator("#btn-stack").click();
await hl.waitForTimeout(200);
check("the pair is on the stack", (await hl.locator("#stack-list .pair").count()) === 1);
await hl.locator("#stack-list .pair .rs").click();
await hl.waitForTimeout(300);
check("the stack's resize button arms the mode too",
  await hl.evaluate(() => document.body.classList.contains("resizing")));
await selectText("gamma");
await hl.keyboard.press("Enter");
await hl.waitForTimeout(300);
state = await marksState();
check("committing shrinks the extent", state.quotes[0] === "gamma", state.quotes[0]);
check("through exactly one resize, minting nothing",
  state.resizes === 1 && state.count === 1, `${state.resizes}/${state.count}`);
check("the pair keeps its stack slot", state.stack === 1);
check("the mode ends on commit",
  !(await hl.evaluate(() => document.body.classList.contains("resizing"))));
check("and the document paints the new extent",
  (await hl.locator("mark.hl").first().textContent()) === "gamma",
  await hl.locator("mark.hl").first().textContent());

// A resize may not swallow another mark either — that is the same unreachable
// stacking by another route.
await selectText("delta epsilon");
await hl.keyboard.press("Control+H");
await hl.waitForSelector("#annot-overlay.open");
await hl.locator("#annot-text").fill("and this?");
await hl.locator("#annot-save").click();
await hl.waitForTimeout(250);
await hl.locator(`mark.hl`).first().click();
await hl.waitForSelector("#annot-overlay.open");
await hl.locator("#annot-resize").click();
await hl.waitForTimeout(150);
await selectText("gamma delta epsilon");
await hl.keyboard.press("Enter");
await hl.waitForTimeout(250);
state = await marksState();
check("a resize onto another mark is refused", state.resizes === 1, `${state.resizes}`);
check("and the mode stays armed for another try",
  await hl.evaluate(() => document.body.classList.contains("resizing")));
await hl.keyboard.press("Escape");

// --- images and the viewer ---
// A page of its own because it needs a document with real `<img>` elements in
// it, and the image has to actually decode: `measureImage` reads
// `naturalWidth`, and the viewer's fit is computed from it, so a stub that
// handed back an unloadable URL would assert nothing about either.
//
// 120x60, deep pink, inline. Small enough to sit in this file and big enough
// that fit-to-window is 1:1, which is what makes the "0 means fit" check below
// distinguishable from "0 means 100%".
const PNG =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAHgAAAA8CAIAAAAiz+n/AAAAhklEQVR4nO3QAQkAIADAMCMax4j" +
  "GsoXCHTzA2dhz6ULj+cEngQbdCjToVqBBtwINuhVo0K1Ag24FGnQr0KBbgQbdCjToVqBBtwINuhVo0K1Ag24FGnQr0KBbgQbd" +
  "CjToVqBBtwINuhVo0K1Ag24FGnQr0KBbgQbdCjToVqBBtwINuhVo0K0Oz9/0hg2WDpYAAAAASUVORK5CYII=";

const pix = await newPage();
pix.on("pageerror", (e) => results.push("FAIL pageerror (images): " + e.message));
pix.on("console", (m) => {
  if (m.type() === "error") results.push("FAIL console.error (images): " + m.text());
});
await pix.addInitScript(({ base, png }) => {
  window.__PNG__ = png;
  const tree = {
    name: "repo", is_dir: true, path: "/repo", rel: "",
    children: [{ name: "doc.md", is_dir: false, path: "/repo/doc.md", rel: "doc.md", children: [] }],
  };
  // Three images: one inside the repo, one that climbs out of it with `../`,
  // and one already absolute in a scheme of its own. What each is *for* is the
  // assertion — the first must be rewritten, the second must be refused, the
  // third must be left exactly as the document wrote it.
  const body =
    `<p><img src="img/pic.png" alt="a picture"></p>` +
    `<p><img src="../../etc/secret.png" alt="escapee"></p>` +
    `<p><img src="data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==" alt="inline"></p>`;
  window.__TAURI__ = {
    core: {
      convertFileSrc(path) {
        (window.__ASSET__ ||= []).push(path);
        return path.endsWith("pic.png") ? window.__PNG__ : "asset://localhost/" + encodeURIComponent(path);
      },
      async invoke(cmd) {
        switch (cmd) {
          case "perf_enabled": return false;
          case "repo_info": return { root: "/repo", name: "repo", display: "~/repo" };
          case "get_keymap": return {
            palette: "Ctrl+F", highlight: "Ctrl+H", settings: "Ctrl+,",
            toggle_stack: "Ctrl+O", toggle_outline: "Ctrl+I", toggle_tree: "Ctrl+B",
            toggle_view: "Ctrl+M", find: "/", quick_highlight: true,
          };
          case "get_theme": return { css: base, mode: "system", scheme: "dark", syntax_theme: null };
          case "get_ui": return {
            tree_width: 260, stack_width: 280, pane_width: 380, pane_height: 240,
            menubar: false, titlebar: false, titlebar_fade: false, zoom: 100,
          };
          case "initial_file": return "/repo/doc.md";
          case "render_markdown": return body;
          case "list_markdown_files": return tree;
          case "get_highlights": case "reanchor": case "get_stack": return [];
          default: return null;
        }
      },
    },
    event: { async listen() { return () => {}; } },
  };
}, { base, png: PNG });
await pix.goto(pathToFileURL(join(UI, "index.html")).href);
await pix.waitForSelector("#content img");
await pix.waitForTimeout(300);

check(
  "a relative image resolves against the repo root, through the asset protocol",
  (await pix.evaluate(() => window.__ASSET__ || [])).includes("/repo/img/pic.png"),
  JSON.stringify(await pix.evaluate(() => window.__ASSET__ || [])),
);
check(
  "and it is the asset URL that lands on the element — never file:",
  await pix.evaluate(() => {
    const src = document.querySelector('#content img[alt="a picture"]').getAttribute("src");
    return src && !src.startsWith("file:");
  }),
);
check(
  "an image that climbs out of the repo loses its src entirely",
  await pix.evaluate(() => !document.querySelector('#content img[alt="escapee"]').hasAttribute("src")),
);
check(
  "and it was never handed to the protocol either",
  !(await pix.evaluate(() => window.__ASSET__ || [])).some((p) => p.includes("secret")),
);
check(
  "a data: image is left alone",
  await pix.evaluate(() =>
    document.querySelector('#content img[alt="inline"]').getAttribute("src").startsWith("data:")),
);

// Measurement, which is what lets an image scale with the prose. The refused
// one must *not* be measured: `width: calc(0px * z)` would collapse its alt
// text to nothing.
check(
  "a loaded image records its natural width",
  (await pix.evaluate(() => document.querySelector('#content img[alt="a picture"]').dataset.w)) === "120",
);
check(
  "a refused image is not measured",
  await pix.evaluate(() => !document.querySelector('#content img[alt="escapee"]').dataset.w),
);
const shotWidth = () =>
  pix.evaluate(() =>
    Math.round(document.querySelector('#content img[alt="a picture"]').getBoundingClientRect().width));
check("and is drawn at that size unzoomed", (await shotWidth()) === 120, String(await shotWidth()));
await pix.evaluate(() => applyZoom(200));
await pix.waitForTimeout(120);
check("and grows with the document zoom", (await shotWidth()) === 240, String(await shotWidth()));
await pix.evaluate(() => applyZoom(100));
await pix.waitForTimeout(120);

// The viewer.
await pix.locator('#content img[alt="a picture"]').click();
await pix.waitForSelector("#lightbox.open");
check("clicking an image opens the viewer", await pix.locator("#lightbox.open").isVisible());
check(
  "at fit, which for an image smaller than the window is 1:1",
  (await pix.locator("#lb-pct").textContent()) === "100%",
  await pix.locator("#lb-pct").textContent(),
);
check(
  "on the src the document already had — the viewer resolves nothing of its own",
  await pix.evaluate(() =>
    document.getElementById("lightbox-img").getAttribute("src") ===
    document.querySelector('#content img[alt="a picture"]').getAttribute("src")),
);

const pixAccel = (await pix.evaluate(() => document.body.classList.contains("mac"))) ? "Meta" : "Control";
await pix.keyboard.press(`${pixAccel}+=`);
await pix.waitForTimeout(120);
check(
  "the zoom keys reach the image while it is open",
  (await pix.locator("#lb-pct").textContent()) === "125%",
  await pix.locator("#lb-pct").textContent(),
);
check(
  "and leave the document behind it alone",
  (await pix.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--zoom").trim())) === "1",
);
await pix.keyboard.press(`${pixAccel}+0`);
await pix.waitForTimeout(120);
check("and 0 is back to fit rather than to 100%", (await pix.locator("#lb-pct").textContent()) === "100%");

// Every binding below the viewer is suspended while it is up: they all act on a
// document it is covering.
await pix.keyboard.press("Control+B");
await pix.waitForTimeout(120);
check(
  "the reader's keys do not reach through the viewer",
  await pix.evaluate(() => document.body.classList.contains("nav-collapsed")),
);

await pix.keyboard.press("Escape");
await pix.waitForTimeout(150);
check("Escape closes it", !(await pix.locator("#lightbox.open").isVisible()));
check(
  "and drops the decoded bitmap rather than holding it for the session",
  await pix.evaluate(() => !document.getElementById("lightbox-img").hasAttribute("src")),
);
check(
  "and view mode is not what Escape ended",
  await pix.evaluate(() => !document.body.classList.contains("view-mode")),
);

await browser.close();
console.log(results.join("\n"));
const failed = results.filter((r) => r.startsWith("FAIL")).length;
console.log(`\n${results.length - failed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
